# cratetorrent

Cratetorrent is an experimental Torrent client written in Rust. The name is a
homage to the C++ [libtorrent](https://github.com/arvidn/libtorrent) library,
from which many lessons were learned when I first wrote my torrent engine in
C++.


## Features

The following features are currently supported:
- Single torrent downloads or uploads, with
- an arbitrary number of peer connections.
- Peers may be specified by their address, or if the torrent's metainfo file
  contains trackers, peers are requested from these trackers.

An iso of Ubuntu 20.04 LTS (around 2.8 GB) is downloaded in about
5 minutes at an average download rate of 10 MPbs on my fairly slow internet
connection, which indicates that performance is acceptably good right out of the
gate. More optimizations are expected.

Features are continuously added, see the [project
milestones](https://github.com/mandreyel/cratetorrent/issues/26).

Eventually, I hope to develop cratetorrent into a full-fledged BitTorrent engine
library that can be used as the engine underneath torrent clients. This means
that features supported by popular clients (such as DHT, magnet links,
BitTorrent protocol 2, stream encryption, and others) will be supported by
cratetorrent in the future.


## Project structure

The project is split up in two:
- the `cratetorrent` library, that defines most of the functionality,
- and a `cratetortent-cli` binary for downloading torrents via the CLI.


## How to run

Tested on stable Rust 1.48.

**NOTE**: requires Linux! This is because file IO is done using the
[`pwritev(2)`](https://linux.die.net/man/2/pwritev) and
[`preadv(2)`](https://linux.die.net/man/2/preadv) APIs for optimal performance.
In the future, API shims for Windows and Darwin may be supported, but at the
moment there is no capacity to do this.

### Binary

The CLI binary is currently very basic, but you can connect to a seed by
running the following from the repo root:
```
cargo run --release -p cratetorrent-cli \
    --listen 0.0.0.0:50051 \
    --seeds 192.168.0.10:50051,192.168.0.172:49985 \
    --metainfo path/to/mytorrent.torrent \
    --download-dir ~/Downloads
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
    --env LISTEN="${listen_addr}" \
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

Cratetorrent is well tested to ensure correct functionality. It includes:
- an exhaustive suite of inline unit tests,
- and integration tests of various downloads and uploads, in the [integration
tests folder](tests).


## Design

The cratetorrent design is documented in the [design doc](DESIGN.md). This
mostly concerns developers of cratetorrent, as it contains fairly low-level
descriptions.
