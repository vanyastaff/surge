use criterion::{criterion_group, criterion_main, Criterion};
use surge_core::Graph;

fn typical_flow_toml() -> &'static str {
    include_str!("../tests/fixtures/graphs/linear-with-review.toml")
}

fn toml_roundtrip(c: &mut Criterion) {
    let toml_s = typical_flow_toml();
    c.bench_function("toml_roundtrip_typical_flow", |b| {
        b.iter(|| {
            let g: Graph = toml::from_str(criterion::black_box(toml_s)).unwrap();
            let s = toml::to_string(&g).unwrap();
            criterion::black_box(s)
        })
    });
}

criterion_group!(benches, toml_roundtrip);
criterion_main!(benches);
