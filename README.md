# cratetorrent

Cratetorrent is an experimental Torrent client written in Rust. The name is a
homage to the C++ [libtorrent](https://github.com/arvidn/libtorrent) library,
from which many lessons were learned when I first wrote my torrent engine in
C++.


## Features

Single download of a torrent with multiple peer connections, if given their
addresses and the path to the torrent metainfo. Downloads are relatively fast
with various throughput optimizations. However, there are no multiple torrents,
no seeding, or any other feature you might expect from a full-fledged BitTorrent
library.

Features are continuously added, please see the [project
milestones](https://github.com/mandreyel/cratetorrent/issues/26). Eventually, I
hope to develop cratetorrent into a full-fledged BitTorrent engine library that
can be used as the engine underneath torrent clients, so this means that
features supported by popular clients (such as DHT, magnet links, BitTorrent
protocol 2, stream encryption, and others) will be supported by cratetorrent
too.


## Project structure

The project is split up in two:
- the `cratetorrent` library, that defines most of the functionality,
- and a `cratetortent-cli` binary for downloading torrents via the CLI.


## How to run

Tested on stable Rust (1.43).

### Binary

The CLI binary is currently very basic, but you can connect to a seed by
running the following from the repo root:
```
cargo run --release -p cratetorrent-cli
```

### In Docker

A Dockerfile is also provided for the CLI. To build the docker image, first
  build the binary (also from the repo root):
```
cargo build --release -p cratetorrent-cli
```
Then build the image:
```
docker build --tag cratetorrent-cli .
```
And finally run it:
```
docker run \
    -ti \
    --env SEED="${seed_addr}" \
    --env METAINFO_PATH="${metainfo_cont_path}" \
    --env RUST_LOG=cratetorrent=trace,cratetorrent_cli=trace \
    --mount type=bind,src="${metainfo_path}",dst="${metainfo_cont_path}" \
    cratetorrent-cli
```
where `seed_addr` is the IP and port pair of a seed, `metainfo_path` is the path
of the torrent file on the host, and `metainfo_cont_path` is the
path of the torrent file mapped into the container.


## Tests

Cratetorrent is well tested to ensure correct functionality. It includes an
exhaustive suite of unit tests verifying the correctness of each part of the
code base, defined inline in the Rust source code.

There is also a host of integration tests for verifying the functionality of the
whole engine, ranging from testing the download of a single file through a
single connection, through downloading a torrent from several peers, to seeding
to other peers, stress testing, and others, added simultaneously with
functionality added. To see more, please see the [integration tests
folder](tests).


## Design

The design and development of cratetorrent is, as much as possible, documented
in the [design doc](DESIGN.md). You will find (fairly low-level) information
about the _current_ he architecture of the code and rationale for the design
decisions that have been taken. It is continuously updated as development, as
much as possible, is driven by well-defined feature specification and subsequent
(code) design specification before any code is written.


## Research

While the thoughts behind the current state of the implementation are stored
in the design doc, thoughts and research on future functionality is stored in
the [research doc](RESEARCH.md).
