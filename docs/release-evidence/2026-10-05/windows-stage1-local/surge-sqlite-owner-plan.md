# Revised retained SQLite pool ownership (precode review)

## Evidence invalidating the manager-only plan

Pinned r2d2 0.8.10 src/lib.rs:175 declares SharedPool fields config, manager,
internals, cond. Normal Rust field destruction releases manager before internals'
idle SQLite connections. An Arc captured only by manager.with_init therefore
cannot establish retention through final SQLite close. SQLite clientdata is not
an alternative: bundled libsqlite3-sys 0.30.1 sqlite3.c:181069 invokes its
destructors before sqlite3LeaveMutexAndCloseZombie reaches sqlite3BtreeClose at
181167. No clientdata or close-hook ownership workaround will be implemented.

## Accepted ownership and public API contract

The owning crate is surge-persistence. On Windows, RetainedConnection is exported
as OwnedSqliteConnection and declares connection: Option<Connection> before an
optional Arc<SqliteNamespaceOwner>. All disk connections carry that Arc. The
wrapper performs checked final close, as detailed below. Ordinary Rust field
order is not the close guarantee.

OwnedSqliteConnectionManager retains an independent Arc plus the real
r2d2_sqlite manager and a private initializer callback. Its associated Connection
is RetainedConnection. connect verifies the namespace, opens through the actual
manager, immediately wraps the raw connection, then invokes initialization.
is_valid and has_broken delegate through a crate-private mutable accessor. No
callback runs inside the underlying manager. Both file and with_init constructors
are crate-private. Windows root SqliteConnectionManager aliases this manager;
Unix reexports the unchanged r2d2_sqlite type. External Windows concrete pool
types change, while inferred ordinary SQLite calls continue through immutable
Deref. Public mutable extraction is unavailable.

Registry and run reader pool constructors and all internal manager imports use
the root manager export. WorkerProbe uses the manager associated Connection type.
Store, MemoryStore, writers, migrations, refusal delivery, and read-only consumers
use the same checked wrapper on Windows. Private native internals never escape.

Public intake_outbox entry points enqueue_terminal, claim, and acknowledge now
borrow &Connection and begin real immediate Transaction::new_unchecked operations.
SQLite continues to reject nested BEGIN; a regression test requires rejection to
preserve the outer transaction and leave both ticket state and outbox unchanged.
Other mutable raw access remains confined to audited persistence implementation.

## Accepted immutable borrowing boundary

Public immutable Deref to
Connection remains; there is NO public DerefMut, AsMut<Connection>, into_inner,
mutable raw accessor or arbitrary callback receiving &mut Connection. Public
transaction(&mut self) and transaction_with_behavior(&mut self, behavior) forward
to the real connection and return Transaction<'_> whose borrow retains the owned
wrapper. Other immutable Connection methods remain available through Deref.

A crate-private `connection_mut(&mut self) -> &mut rusqlite::Connection` is used
only by audited internal manager validation, migration/callback adaptation and
the native test that deliberately retires every actual disk connection. The
manager's file and with_init constructors are both crate-private; external code
receives pools exclusively through the existing public registry constructor and
cannot install a raw mutable callback. No new public mutation escape exists.

The dedicated derived-pool oracle retires each complete checked-out wrapper by
replacing it with a crate-private in-memory wrapper carrying no namespace Arc,
then explicitly closes the original owned disk connection. Replacing only the
raw SQLite connection would leave per-connection namespace owners alive and
mask a missing manager fence. Manager fencing is independently observed after
all disk connections and their wrappers retire, then complete final release is
observed after the pool and in-memory slots drop.

The existing new internal RetainedConnection used by readonly opens follows
this same no-public-DerefMut design. On Unix it remains crate-private and forwards
its narrow transaction methods; the Unix public pool manager/Connection types
remain unchanged. Current consumers needing mutable methods are audited to call
transaction forwarding or the crate-private accessor, never weaken ownership.

## Checked final close and unrecoverable Drop policy

Field order alone is insufficient: pinned rusqlite 0.32.1 InnerConnection::drop
ignores a failed sqlite3_close, and SQLITE_TRACE_CLOSE runs before the SQLite
busy check. A safe forgotten Statement can trigger SQLITE_BUSY. All Windows
filesystem-backed connection owners therefore use one checked owning wrapper,
not raw Connection plus a separate guard field.

Concrete wrapper: `connection: Option<Connection>` and retained namespace Arc.
A private constructor receives the already-created raw Connection and Arc
immediately, before any PRAGMA, migration, callback or caller SQLite operation.
No raw Connection escapes public APIs. Option::None is only the transient consumed
close state; public immutable deref/narrow transaction methods never observe it.
An impossible empty access aborts instead of fabricating another connection.

Public `close(mut self) -> Result<(), (Box<Self>, rusqlite::Error)>` takes the actual
Connection and calls Connection::close. On failure the returned raw Connection
is restored into the same wrapper and the full owning wrapper is returned with
the SQLite error. Thus SQLITE_BUSY retains all native ownership and callers can
retry after settling outstanding work. A successful explicit close releases
namespace only after SQLite confirmed close. This does not weaken the public
immutable-only borrowing interface described above.

Drop has an explicit fatal protected-owner disposition: run its checked close
inside surge_process::owner_panic::abort_on_owner_panic while the namespace stays
in the outer wrapper. If close returns an owning raw connection plus error,
retain that connection until immediate process abort; emit only a static failure
classification / SQLite error code (no SQL/payload/path bytes). The protected
panic scope makes logger/close panic abort with the namespace still retained.
No ignore-error, raw sqlite3_close_v2 workaround, detached reaper, ordinary
panic/unwind, or returning from Drop after a failed close is allowed. Fatal
termination is intentional: forgotten outstanding SQL state cannot release
ownership and let the host claim safe continuation. The shared panic hook is
installed by the owning-wrapper constructor, covering standalone Store callers
as well as Storage callers. Normal successful close remains ordinary behavior.

Apply the same Windows checked wrapper to:
- registry migration connection BEFORE PRAGMA/migration and every managed pool
  connection;
- run creation migration and upgrade connection BEFORE PRAGMA/migration;
- writer task connection BEFORE its PRAGMA, through its explicit close path;
- readonly inspection/control/recovery/verification/usage connections;
- owned-flow refusal delivery JournalOwner connection (plus its independent
  WriterLease namespace for writer admission); and
- Store and MemoryStore connection fields and their initialization paths.
  In-memory Store constructors can use a private wrapper with no filesystem
  namespace, but still use checked close. Unix raw Store/manager/writer concrete
  types remain unchanged; the new internal readonly helper can use ordinary
  Unix close semantics because it grants no native ownership capability.

Every existing raw connection opening in persistence has been inventoried; tests
that directly open SQLite for independent assertions stay independent fixtures.
Do not claim complete namespace ownership if any production connection remains
as an unchecked raw Connection outside a protected wrapper.

Additional decisive tests:
- Genuine busy explicit-close and fatal-Drop child probe: open a real owned
  SQLite database, prepare an actual statement, then safely std::mem::forget the
  Statement. Explicit wrapper.close must return SQLITE_BUSY plus the owning
  wrapper; independently observe Weak namespace upgrade / native rename refusal
  while that returned wrapper is alive. Dropping it must cause the specified
  fatal child-process termination, bounded and joined by the parent harness.
  The test does not call raw sqlite3_close/v2 or manually invent a busy error.
  A readiness/observation receipt records that the real busy error and retained
  identity were observed before expected abort. This is not a skipped test.
- Successful explicit close: actual WAL transactions, close every owner normally,
  verify final WAL/SHM deletion and namespace Weak expiry / successful rename.
- Init-error path: checked wrapper is established before invoking PRAGMAs/init,
  so any callback failure closes via the same checked path with Arc retained.
  No inner r2d2_sqlite with_init can own the init callback: the custom manager
  stores and invokes its crate-private callback only AFTER wrapping the raw
  successful Connection. inner manager is a file opener without user callbacks.
- SQLITE_TRACE_CLOSE is supplemental evidence only; it cannot by itself prove
  successful final close. Explicit close result, namespace expiry and sidefile
  behavior carry the acceptance verdict.

## Native acceptance and limits

The dedicated derived-pool oracle retires entire disk connection wrappers as
described above, closes each original connection, and then proves the independent
manager still blocks namespace rename until the pool settles. Its standalone fixture repair
must be committed before changing that body for the new manager.

New native tests cover two real writable connections and WAL/SHM cleanup after
confirmed closes; a real failing SQL initializer with final owner release; a
safely forgotten statement producing actual SQLITE_BUSY, retaining ownership on
explicit close, and causing bounded child-process fatal Drop settlement. They
must execute in the dedicated Windows standard-user gate. Exact Windows module
cross compilation and host checks are compiler evidence only. Stage1 remains
open until full native behavior and independent source reviews pass. Private and
preparation unsupported guards stay closed through stages 1–4.
