# cratetorrent

Cratetorrent is an experimental Torrent client written in Rust. The name is a
homage to the C++ [libtorrent](https://github.com/arvidn/libtorrent) library,
from which many lessons were learned when I first wrote my torrent engine in
C++.


## Goals

1. Perform a single download of a file with a single BitTorrent client if given
   the address of a seed and the necessary torrent meta-information. No multiple
   torrents, no seeding, no optimizations, or any other feature you might expect
   from a full-fledged BitTorrent library.
2. Implement metainfo parsing.
3. Download an directory of files using a single peer connection.
4. Download a torrent using multiple connections.
5. Seed a torrent.


## Integration tests

There is a whole suite of integration tests to ensure that cratetorrent works as
expected, with various test cases for various use cases. To see more, please see
the tests folder [readme](tests/README.md).


## Design

To be expanded.
