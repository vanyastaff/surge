# Role 29 — Deployment and supervised daemon startup

## Findings and baseline evidence
- P1 daemon startup/server error was logged but main returned exit0, hiding failed readiness from supervisors. Actual baseline `target/release/surge-daemon` isolated subprocess with directory occupying `$SURGE_HOME/daemon/daemon.sock` emitted server error and exited0; expected1 assertion failed. Evidence `/tmp/surge-r29-red.log`.
- P1 daemon unconditionally unlinked existing path and left daemon parent0755. Isolated baseline binary replaced regular user file with socket, then graceful termination removed it. Evidence `/tmp/surge-r29-security-red.log` (user data preserved=false, parent0755).
- Platform docs explicitly describe configured Linux Ubuntu24.04, macOS15 Intel/ARM, WindowsMSVC release matrix and distinguish CI configuration from native validation. No external deployments or native Windows/Linux validation performed.

## Changes (verification pending)
- main.rs retains spawned server Result, joins completed server and returns exit1 for server errors/unexpected task join failures; intentional shutdown grace cancellation remains exit0.
- Removed unconditional main socket unlink.
- New private socket_security.rs: descriptor-open O_NOFOLLOW directory, uid validation, secure ancestor checks, descriptor0700, namespace dev/ino validation; stale path removal only actual owned sockets; postbind0600 fail closed, identity retained RAII socket cleanup only bound inode.
- server.rs delegates secure startup and retains socket guard; lib.rs declares Unix private module.
- New subprocess regressions daemon_startup_exit.rs plus five socket helper unit tests (including native macOS extended ACL grant rejection) covering modes, stale removal, replacement preservation and symlink/directory replacement rejection.

## Checks
- Actual two RED reproductions above performed against preexisting release binary with /tmp sandbox and no user runtime state.
- rustfmt standalone helper/test/main passes.
- Cargo tests/clippy/GREEN not yet executed; coordinator owns queue.
- Independent security review requested through coordinator.

## Security boundary
macOS extended grant ACLs survive chmod0700. Copied the existing persistence descriptor deny-only ACL validator exactly into a private Mac module, with safety contracts; parent descriptor and canonical ancestor descriptors reject grant/unknown/error ACLs. New native Mac test adds a real everyone list/search grant to a disposable TempDir and expects PermissionDenied. GREEN/security review pending. Adversarial same-UID path replacement is outside the user-identity ACL boundary; namespace inode checks prevent cleanup of replacements. Windows named-pipe DACL native evidence not performed locally.

Verdict: NEEDS WORK pending GREEN and independent review, native ACL regression and full security review.
