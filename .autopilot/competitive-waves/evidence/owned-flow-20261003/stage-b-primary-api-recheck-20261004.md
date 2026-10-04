# StageB primary API recheck, 2026-10-04

Root inspected installed locked dependency sources and current adapters, read-only.
No source permission or executable proof follows.

- Root Cargo uses nix0.29 signal/fs/user/dir; poll feature must be explicit.
  Installed nix0.29 fcntl and read accept RawFd safely; write uses AsFd; PollFd
  retains a borrowed descriptor lifetime. poll is a safe wrapper around libc poll.
  A raw numeric descriptor argument still requires original OwnedFd owner retained
  across readiness/check/write; safe signature alone does not solve ownership.
- ACP sdk_v1 currently caps incoming lines at 1MiB and queues31 frames plus one
  ordered dispatch/lookahead. send_frame waits for channel capacity BEFORE parsing.
  Private incoming result capture must precede that wait without uncapped buffers.
- rmcp1.6 AsyncRw JsonRpcMessageCodec default sets max_length=usize::MAX.
  New owned finite MCP frame capacity is a real resource-policy/compatibility change,
  not preservation of a nonexistent old finite limit. Codec exposes max length,
  CRLF/EOF decoding behavior must be inspected and kept or explicitly revised.
- No generic AsyncWrite readiness gate supplies an actual after-writable-wait syscall
  gate. Exact original descriptor/owner and byte cursor remain concrete StageB work.

Primary files: Cargo.toml; crates/surge-acp/src/sdk_v1.rs and sdk_v1/transport.rs;
installed registry nix-0.29.0/src/{fcntl,poll,unistd}.rs;
rmcp-1.6.0/src/transport/async_rw.rs. Findings sent to owning architect.
