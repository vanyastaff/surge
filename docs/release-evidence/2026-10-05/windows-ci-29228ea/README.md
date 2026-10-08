# Windows CI candidate 29228ea

Revision: `29228ea84b15943522f1c4e3c77642226560b4ba`.
Run: https://github.com/vanyastaff/surge/actions/runs/37698935672

- macOS and Ubuntu test suites and Clippy passed.
- Windows nextest: 3,685 tests run, 2,922 passed, 763 failed, 46 skipped.
  Many failures report `object owner is not trusted`; the refused object is not
  identified by the current error, so the underlying cause remains unresolved.
- Dedicated standard-user native gate: complete-flush probe passed; protected
  new-home probe failed with the same owner refusal. The remaining ten probes
  did not execute. This is not native acceptance.
- Windows Clippy failed on `manual_assert` in the checked-close subprocess test.

Raw job logs are retained as gzip files. Release remains NO-GO.
