# `cratetorrent`

Cratetorrent is an experimental Torrent client written in Rust.

The first goal of this project is to perform a single download of a file with a single
BitTorrent client. No multiple torrents, no seeding, no optimizations, or any
other feature you might expect from a full-fledged library.



## Testing


### Unit tests


### Integration tests

There is a whole suite of integration tests to ensure that cratetorrent works as
expected.

Since the first goal is to download a single file, the tests are designed with
this in mind. The tests create a local Docker network of two BitTorrent peers,
in which one container runs an established torrent client (TODO: libtorrent,
transmisison, deluge, …?), and another runs cratetorrent. These containers are
going to share a Docker virtual network, and the established torrent client is
going to seed a predefined file, while our client is going to try downloading
it.

The integration test script sets up the network with the seed container that
includes the file to be seeded, a leech cratetorrent container which is going to
download the file into a Docker volume mapped directory. The in-progress file
download is going to have the `.part` suffix (this is a feature of many existing
clients), and it is only removed once the download finishes. The tests script is
going to continuously query for the existence of the resulting file, and if such
is found, it concludes the download finished and compares it with the file that
was used for seeding.

#### Questions:
- How to seed an existing file without having to re-download every time?
- How to make tests reproducible (i.e. all assets completely contained in the docker
images)?
- How large a test file to use? In the beginning, we only want to see if a
download occurs, but later a larger file may be necessary to test performance.
However, even the smaller file has to span several BitTorrent pieces so that we
can test the piece download strategy works.


## Design
