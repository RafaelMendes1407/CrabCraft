//! Parser and highlighter benchmarks on a large Rust file.

use criterion::{Criterion, criterion_group, criterion_main};
use rc_syntax::{Lang, highlight_lines, input_edit, parse_now, sample::big_rust};
use rc_text::{Buffer, Change, ChangeSet, EditKind, Selection};
use std::hint::black_box;

fn benches(c: &mut Criterion) {
    let text = big_rust(20_000);
    let buf = Buffer::new(&text);
    let rope = buf.snapshot();
    let tree = parse_now(Lang::Rust, &rope, None).unwrap();

    c.bench_function("full parse 20k lines", |b| b.iter(|| black_box(parse_now(Lang::Rust, &rope, None))));

    // Typing one char in the middle of a method body, then reparsing incrementally.
    let mid = rope.line_to_char(10_000) + 8;
    c.bench_function("incremental reparse after a keystroke", |b| {
        b.iter_batched(
            || {
                let mut buf = buf.clone();
                let edits = buf.apply(&ChangeSet::single(Change::insert(mid, "x")), &Selection::point(mid), &Selection::point(mid + 1), EditKind::Insert);
                let mut old = tree.clone();
                for e in &edits {
                    old.edit(&input_edit(e));
                }
                (buf.snapshot(), old)
            },
            |(rope, old)| black_box(parse_now(Lang::Rust, &rope, Some(&old))),
            criterion::BatchSize::SmallInput,
        )
    });

    c.bench_function("highlight 80 visible lines", |b| {
        b.iter(|| black_box(highlight_lines(Lang::Rust, &tree, &rope, 10_000, 80)))
    });
    c.bench_function("folding ranges 20k lines", |b| b.iter(|| black_box(rc_syntax::folding_ranges(Lang::Rust, &tree, &rope))));
}

criterion_group! {
    name = parse;
    config = Criterion::default().sample_size(30).measurement_time(std::time::Duration::from_secs(3));
    targets = benches
}
criterion_main!(parse);
