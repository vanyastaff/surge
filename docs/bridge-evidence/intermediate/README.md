# Intermediate native CI receipts

These receipts preserve completed failures and an intentionally cancelled run during the Windows workspace implementation. They are historical results at distinct source revisions, not evidence for the final source revision. The initial bridge receipt is retained separately under `../7f937140f84be2df7133c55aeeb2dd01f5b1d5f4/`.

| Workflow | Exact source revision | Conclusion | Saved receipts |
|---|---|---|---|
| [38197983756](https://github.com/vanyastaff/surge/actions/runs/38197983756) | `b9d4736ac5b70a971929c01df50d2ea883dd2622` | `failure` | [status](38197983756/workflow-status.json), [diagnostic excerpts](38197983756/failure-excerpts.log) |
| [38200432380](https://github.com/vanyastaff/surge/actions/runs/38200432380) | `0f7658b3269901c493aab1787a847ba4ff146ecf` | `failure` | [status](38200432380/workflow-status.json), [diagnostic excerpts](38200432380/failure-excerpts.log) |
| [38204718136](https://github.com/vanyastaff/surge/actions/runs/38204718136) | `6f344f2aeb92eeccc1d88f028b6baf6b576a8774` | `failure` | [status](38204718136/workflow-status.json), [diagnostic excerpts](38204718136/failure-excerpts.log) |
| [38207780717](https://github.com/vanyastaff/surge/actions/runs/38207780717) | `0c319b389885b952db07c8df7c2c5fd986099dec` | `cancelled` | [status](38207780717/workflow-status.json), [diagnostic excerpts](38207780717/failure-excerpts.log) |
| [38209359113](https://github.com/vanyastaff/surge/actions/runs/38209359113) | `e1509691ffaa28c55a5ab90359afecb6b3d3b626` | `failure` | [status](38209359113/workflow-status.json), [diagnostic excerpts](38209359113/failure-excerpts.log) |
| [38211302129](https://github.com/vanyastaff/surge/actions/runs/38211302129) | `60dbf290bbc07fa7e48177eac547998c2f9198f5` | `failure` | [status](38211302129/workflow-status.json), [diagnostic excerpts](38211302129/failure-excerpts.log) |

Excerpts are diagnostic selections from the original CI job logs with their UTC timestamps retained. They are not complete workspace logs. Standard-user manifests, when present, preserve their actual status and inventory; an incomplete manifest does not prove a passed probe. The cancelled workflow does not establish a completed Windows workspace or standard-user result.

- [38213954301](https://github.com/vanyastaff/surge/actions/runs/38213954301), `53f16837c4cf1fdadd3f4cfcd2b250725a6a6089`: completed failure; 10/11 jobs passed, Windows workspace 3936 passed / 2 failed / 55 skipped. All three bridge receipts and 48 standard-user probes passed. Failures: probabilistic short-ULID uniqueness assertion and quota fallback timeout without causal diagnostic.

- [38217833160](https://github.com/vanyastaff/surge/actions/runs/38217833160), `5b73c95c2e5721284ae4115b51dbb247ade6a589`: completed failure; 10/11 jobs passed, Windows workspace 3937 passed / 1 failed / 55 skipped. Bridge receipts on all three OS and all 48 standard-user probes passed. Sole failure: completed-gate route fixture submitted an answer immediately after asynchronous Continue admission; response error details were not retained. A reachable active-run/before-waiter registration gap was subsequently reproduced deterministically.

- [38221338334](https://github.com/vanyastaff/surge/actions/runs/38221338334), `826fa56210b7b7dede2d1559ff598b6859ea173d`: completed failure; 10/11 jobs passed. Windows workspace 3937 passed / 2 failed / 55 skipped; bridge3 and standard-user48 passed. Failures: SQLite SHM native identity/security refusal and forged-gate fixture demanding immediate Attention despite admitted ContinueReserved/Busy claim. Earlier gate-readiness regression passed.

- [38226026892](https://github.com/vanyastaff/surge/actions/runs/38226026892), `f86e483d45c139d29ac359e614de505c5cc29bbb`: completed failure; 10/11 jobs passed. Windows workspace 3938 passed / 1 failed / 55 skipped; bridge3 and standard-user48 passed including extended SQLite retirement/substitution oracles. Sole failure: owned Flow MCP cold-host readiness received EOF; trace reported named-pipe accept invalid handle6. Upstream mio1.1.1 NamedPipe connect ownership race found subsequently; precise occurrence attribution remained inference.
