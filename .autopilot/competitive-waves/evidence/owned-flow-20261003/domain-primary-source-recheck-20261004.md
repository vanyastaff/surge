# macOS process observation: primary source recheck

Root read the current Apple XNU implementation on 2026-10-04 while the native
capability amendment remains unreviewed. This is source research, not a native
execution result or approval to change spawn primitives.

The header marks descendant tracking flags unsupported; filter attachment rejects
them. They cannot provide a generic process-tree monitor.
[Apple event.h](https://raw.githubusercontent.com/apple-oss-distributions/xnu/main/bsd/sys/event.h),
[Apple kern_event.c](https://raw.githubusercontent.com/apple-oss-distributions/xnu/main/bsd/kern/kern_event.c).

Requested process event bits accumulate in the filter. Exit sets terminal flags;
a debugger reparenting can suppress exit delivery to the original observer until
reparenting is reconciled. A candidate never-fork proof therefore needs an actual
terminal monitor receipt and uninterrupted identity-bound observation. Root wait
alone, delayed registration or missing event cannot grant closure. This is a
conservative inference from filt_procattach/filt_procevent, not a proven supported
native backend. Supported OS versions and fork/exec/cancellation races still need
actual native tests. [Apple implementation](https://raw.githubusercontent.com/apple-oss-distributions/xnu/main/bsd/kern/kern_event.c).

Cold author test handoff independently verified before main formatting:
`39f3c380d9aea9f93a392b048ecbb58d3136c4465727fb832570481a6590aae5`.
Newfile ALL write/format ownership returned to main. Raw Closed assertion in the
current positive fixture is only a missing-history detector; final acceptance
needs the reviewed genuine whole-domain closure set. No baseline result exists
at this checkpoint. Initial formal repair count remains0/3.
