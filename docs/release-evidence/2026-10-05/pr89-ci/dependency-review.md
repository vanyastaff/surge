# R10 Rust1.99 macro compatibility — final upstream correction; root gates pending

CI RED: /tmp/surge-pr89-ci-clean.log records exactly8 double_must_use errors for eachLinux/Windows/macOSClippy job, compilerexit101. Six in BridgeFacade declarations,2 in ClientCallbacks; macro async-trait0.1.89 always adds bare#[must_use] (installed locked source src/expand.rs:69).

Owned diff only crates/surge-acp/src/bridge/facade.rs and src/sdk_v1.rs. Removed trait-only async_trait annotations, explicitly spelled generated boxedfuture ABI/lifetimes using alreadyavailable futures::future::BoxFuture/LocalBoxFuture. Preserve exact receiver 'life0, generated 'async_trait, 'life0:'async_trait,Self:'async_trait; all ownedargs/outputs, BridgeSend and callbacksnonSend unchanged. Allimplementation macros unchanged, synchronous methods unchanged. Future type remains must_use; no lint suppression/no dependencychange/noCIedit/downgrade.

Reason chosen: adding explicitmust_use message before macro still causesunused_attributes because macroblindlyaddssecond attribute (standalone rustcdenywarnings repro /tmp/r10-must-use-probe.rs rejected). Manualdyncompatibledeclarations avoidgeneratedredundancy without changingruntime behavior or hidingwarnings. Traitgenerator exactlifetimes verified against source and R26 independent maintainer review ACCEPTABLE.

Sequence disclosure: first draft preceded rootexplicitprecode-reviewrequest; independent review obtained beforeCargo/commits. No behavior change intended; existingSDKlifecycle suite and strictCargoClippy are appropriate acceptance, no mirror-tests added.

rustfmt owned2files passed; gitdiff--check passed. Cargo notyetexecuted pendingexclusive slot; no green claim.

## Cross-crate closure update
Root Rust1.99 strict ACP Clippy passed28.86s. Full workspace then exposed2 identical generated double_must_use errors in surge-mcp/writer_observer.rs. Fixed that trait only via alreadyavailable BoxFuture: before_child life0self/life1server bothoutliveasync_trait; child_started life0self. Synchronousbefore_effect default unchanged, all implementation macros retained. R26 independent final source ACCEPTABLE; rustfmt/diffcheckpassed; root owns actual Cargo verification.

Remaining26 macrotraits mapped /tmp/r10-remaining-async-traits.txt; notedited without actualfailure/scopeauthorization. Map covers orchestrator, notify, telegram, daemon, intake. Defaultmethods require exact lazy asyncmove and existinggeneratorSelfSync constraints; no blanket suppression or regexdefault rewrite recommended.

## Final ecosystem RESHAPE (manual drafts withdrawn)
Published async-trait0.1.92 officialtag Cargo.toml+src/expand.rs independently inspected. ExactexpandSHA2563982f195156217844ca78ed7d469d4434925eff6c4f234f9cfd1185f4cd221c5; matchesrootdownload and ZERO must_use occurrences. Traitexpansion no longer insertsbaremust_use. Lifetimeelision,outlives,Selfbounds,boxedSend/nonSend futures preserved; syn3ReceiverKind changes AST handling mechanically, no projectAPI manualdesugar needed. MSRV1.71 vsSurge1.96. Existinglockedproc-macro2+quote remain; newdepsyn3.0.6 alreadylocked. R26 independent conditional maintainerACCEPTABLE.

RestoredONLYourthreeuncommittedmanualtraitdraftstoHEAD (ACPfacade,SDKcallbacks,MCPwriterobserver), nooverlappinguseredits. FinalsourcechangeONLYrootasync-traitmin0.1.92 and correspondingCargo.lock package version/checksum/syn dependency.

Commands: cargo+1.99.0update-pasync-trait--precise0.1.92 exit0; initiallynormalizedunrelatedexistingWindows/heckedges. Atrootinstructionretainedonlyresolvedasync-traitpackageentryinHEADlock and restoredunrelatededges. cargo+1.99.0metadata--locked--offline--format-version1 WITHfullresolveddependencies exit0, output /tmp/r10-async-trait092-full-metadata.json; no furtherlocknormalizationrequired. gitdiffcheckPASS. NoCargo buildsbyR10. RootownsfinalstrictworkspaceClippy1.99+MSRVregressions. Dependencyprovenance/noticesmustrecapture before releasecandidateclaims.

Primarysources: https://raw.githubusercontent.com/dtolnay/async-trait/0.1.92/Cargo.toml and https://raw.githubusercontent.com/dtolnay/async-trait/0.1.92/src/expand.rs .

Publishedcrate archive cache independentlyverified SHA25682f6aeea286b8eb4dd3431a1be1b59d290ace00f5bfd8e2a159bc2a05e2c1667 matcheslockedchecksum. Archiveexpand.rs exactmatchesofficialtagSHA3982f195156217844ca78ed7d469d4434925eff6c4f234f9cfd1185f4cd221c5 andcontainsZERO must_use. Finalchoiceisupstreammacrofix, notmanualtraits; priorsections retain honestinvestigationhistory but no longerdescribe finalimplementation.
