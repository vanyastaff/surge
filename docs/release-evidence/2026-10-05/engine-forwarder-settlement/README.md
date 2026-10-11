# Local engine settlement evidence

This directory preserves actual failing and passing local checks, exact source hashes and reviewed plans. It does not establish full source review, native Windows acceptance or release readiness. See implementation-progress.md and the source-freeze receipts for explicit remaining requirements.

Raw command logs are stored as `.log.gz` so compiler whitespace and trailing blank lines remain byte-for-byte intact. `raw-log-archives.json` records both archive and uncompressed SHA-256. Earlier immutable receipts retain the original `.log` basename and its uncompressed hash; decompress the corresponding archive to recover those exact bytes. Original working copies remain under ignored `target/ci-repair-evidence/`. No log was normalized to satisfy a source whitespace check.
