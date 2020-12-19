# cratetorrent

Cratetorrent is a Rust crate implementing the BitTorrent version 1 protocol. It
can be used as a library and also provides a simple example CLI torrent app.

![](assets/partial-download.gif)

The name is a homage to the C++
[libtorrent](https://github.com/arvidn/libtorrent) library, from which many
lessons were learned when I first wrote my torrent engine in C++.


## Features

- Torrent downloads or uploads, with
- an arbitrary number of peer connections.
- Peers may be specified by their address, or if the torrent's metainfo file
  contains HTTP trackers, peers are requested from these trackers.
- Performance is decent:
  > On my fairly slow internet connection with peak download rates of about 9 MBps,
  Ubuntu 20.04 LTS (~2.8 GB) is downloaded in about 5 minutes at a download rate
  of 8-9 MPbs, that is, almost fully utilizing the link.

Features are continuously added, see the [project
milestones](https://github.com/mandreyel/cratetorrent/issues/26).

Eventually, I hope to develop cratetorrent into a full-fledged BitTorrent engine
library that can be used as the engine underneath torrent clients. This means
that features supported by popular clients (such as DHT, magnet links,
BitTorrent protocol 2, stream encryption, and others) will be supported by
cratetorrent in the future.


## Example

```rust
use cratetorrent::prelude::*;
                                                                             
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // spawn the engine with a default config
    let conf = Conf::new("/tmp/downloads");
    let (engine, mut alert_rx) = engine::spawn(conf)?;
                                                                             
    // parse torrent metainfo and start the download
    let metainfo = std::fs::read("/tmp/imaginary.torrent")?;
    let metainfo = Metainfo::from_bytes(&metainfo)?;
    let torrent_id = engine.create_torrent(TorrentParams {
        metainfo,
        // tell the engine to assign a randomly chosen free port
        listen_addr: None,
        mode: Mode::Download { seeds: Vec::new() },
        conf: None,
    })?;
                                                                             
    // listen to alerts from the engine
    while let Some(alert) = alert_rx.next().await {
        match alert {
            Alert::TorrentStats { id, stats } => {
                println!("{}: {:#?}", id, stats);
            }
            Alert::TorrentComplete(id) => {
                println!("{} complete, shutting down", id);
                break;
            }
            _ => (),
        }
    }
                                                                             
    // Don't forget to call shutdown on the engine to gracefully stop all
    // entities in the engine. This will wait for announcing the client's
    // leave to all trackers of torrent, finish pending disk and network IO,
    // as well as wait for peer connections to cleanly shut down.
    engine.shutdown().await?;
                                                                             
    Ok(())
}
```

## Project structure

The project is split up in two:
- the `cratetorrent` library, that defines most of the functionality,
- and a `cratetorrent-cli` binary for downloading torrents via the CLI. Note,
  however, that this is extremely simple at present, currently only used for
  integration testing.


## How to run

Tested on stable Rust 1.48.

**Requires Linux!**

This is because file IO is done using the
[`pwritev(2)`](https://linux.die.net/man/2/pwritev) and
[`preadv(2)`](https://linux.die.net/man/2/preadv) APIs for optimal performance.
In the future, API shims for Windows and Darwin may be supported, but at the
moment there is no capacity to do this.

### Binary

The CLI binary is currently very basic, but you can perform downloads either by
directly connecting to seeds or if the torrent is backed by a HTTP tracker.

Run the following from the repo root:
```
cargo run --release -p cratetorrent-cli \
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
