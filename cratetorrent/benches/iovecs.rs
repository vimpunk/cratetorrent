#[macro_use]
extern crate lazy_static;

use {
    cratetorrent::iovecs::{IoVec, IoVecs},
    criterion::{black_box, criterion_group, criterion_main, Criterion},
};

lazy_static! {
    // the end offsets of the conceptual files
    static ref FILE_ENDS: Vec<usize> = vec![
        10, 25, 34, 40, 55, 71, 85, 96
    ];

    // the blocks of data to write
    static ref BLOCKS: Vec<Vec<u8>> = vec![
        (0..16).collect::<Vec<u8>>(),
        (16..32).collect::<Vec<u8>>(),
        (32..48).collect::<Vec<u8>>(),
        (48..64).collect::<Vec<u8>>(),
        (64..80).collect::<Vec<u8>>(),
        (80..96).collect::<Vec<u8>>(),
    ];
}

pub fn iovecs_benchmark(c: &mut Criterion) {
    c.bench_function("fragmented files write", |b| {
        b.iter(|| {
            let mut bufs: Vec<_> = BLOCKS
                .iter()
                .map(Vec::as_slice)
                .map(IoVec::from_slice)
                .collect();
            let mut buf_slices = bufs.as_mut_slice();

            // iterate through files and get the corresponding slice of iovecs
            let mut prev_file_end = 0;
            for file_end in FILE_ENDS.iter() {
                let file_len = file_end - prev_file_end;
                let mut iovecs =
                    IoVecs::bounded(buf_slices, black_box(file_len));
                // here we would write to file
                // ..
                // then we need to advance the write buffer cursor
                iovecs.advance(file_len);
                // then we get the buffers past the bound
                buf_slices = iovecs.into_tail();
                prev_file_end = *file_end;
            }
        })
    });
}

criterion_group!(benches, iovecs_benchmark);
criterion_main!(benches);
