# Windows-only precursor lint repair

Native [Clippy job](https://github.com/vanyastaff/surge/actions/runs/37687504548/job/113018996978) on `965ac42` compiled the real Windows test source and reported two lints: `similar_names` for `owned` versus `owner`, and `assert_is_empty` for a nonempty string assertion. This is a lint failure, not behavioral RED.

The reviewed repair renames the allocation binding to `descriptor_allocation` and uses `assert_ne!(disk_path, "")`. No suppression or oracle change. Direct rustfmt (edition 2024) and diff-check passed. Fresh spec/security reviews accepted test hash `84a7197ae85c1469956f84d0745fd82e4d3e848cda43a74c9a48d24cb6e8fcbe`. Native repaired Clippy and actual missing-backend behavior remain pending. The unmodified native log is retained compressed and source/log hashes are bound in the manifest.
