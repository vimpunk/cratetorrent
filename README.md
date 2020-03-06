# cratetorrent

Cratetorrent is an experimental Torrent client written in Rust. The name is a
homage to the C++ [libtorrent](https://github.com/arvidn/libtorrent) library,
from which many lessons were learned when I first wrote my torrent engine in
C++.


## Project structure

The project is split up in two:
- the `cratetorrent` library, that defines most of the functionality,
- and a `cratetortent-cli` binary for downloading torrents via the CLI.


## Goals

1. Perform a single download of a file with a single peer connection if given
   the address of a seed and the path to the torrent metainfo. No multiple
   torrents, no seeding, no optimizations, or any other feature you might expect
   from a full-fledged BitTorrent library.
2. Download a directory of files using a single peer connection.
3. Download a torrent using multiple connections.
4. Seed a torrent.

And more milestones to be added later. Eventually, I hope to develop
cratetorrent into a full-fledged BitTorrent engine library that can be used as
the engine underneath torrent clients.


## Tests

Cratetorrent is well tested to ensure correct functionality. It includes an
exhaustive suite of unit tests verifying the correctness of each part of the
code base, defined inline in the Rust source code.

There is also a host of integration tests for verifying the functionality of the
whole engine, ranging from testing the download of a single file through a single
connection, through downloading a torrent from several peers, to seeding to
other peers, stress testing, and others. To see more, please see the
[integration tests folder](tests).


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
