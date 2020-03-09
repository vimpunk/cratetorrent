# Integration tests

Each integration test case tests a different type of expected functionality of
cratetorrent.

In general all tests use Docker containers and a Docker virtual network to
simulate a primitive network of peers (later on this may be expanded so that
peers are not on a LAN), and one or more of the containers will run established
torrent clients against which to test cratetorrent's protocol compliance, i.e.
to make sure that cratetorrent works with torrent clients used in the wild.

To ensure reproducible test results, it is always the same file that is going to
be downloaded or seeded, and external clients are not going to be involved, and
the client is going to connect to the seed directly, without the involvement of
BitTorrent trackers or the DHT, that may introduce variable test runs. These are
supposed to be tested separately at a later time point, when functionality is
added.

Moreover, various file sizes are going to be tested to ensure that
cratetorrent works correctly with small and large files, with different piece
sizes and other parameters.


## Prerequisites

To run the tests, you first first need to build the `cratetorrent-cli` binary
and its corresponding docker image. For instructions, see the project readme.


## Single file download

```
./test_single_connection_download.sh
```

The test [script](test_single_connection_download.sh) creates a local Docker
network of two BitTorrent peer containers, in which one container runs the well
established Transmission torrent client that has a
[CLI](https://manpages.ubuntu.com/manpages/bionic/man1/transmission-cli.1.html)
as well as a pre-packaged [Docker
image](https://hub.docker.com/r/linuxserver/transmission/). Another container
runs the cratetorrent test binary.

The Transmission torrent client, with a pre-defined Docker IP address, seeds the
predefined file, while our client tries to download the file.

The test binary takes as argument the address of the single seed, and the
various torrent details that are normally found in the torrent metainfo file
(the one with the `.torrent` suffix). The reason these are given as command line
arguments to the binary by the test runner is that metainfo parsing is not yet
implemented, as the very first goal is to just connect to the seed and download
a file.

There is a predefined 1MiB file,
[`1mb-test.txt`](assets/transmission/downloads/completed/1mb-test.txt), along
with the torrent [metainfo file](assets/transmission/watch/1mb-test.txt) both
created using the Transmission CLI. (For instructions, see
[below](#set-up-test-environment)).

The integration test script sets up the network with the seed container, a leech
cratetorrent container which is going to download the file into a Docker bind
mounted directory. The in-progress file is going to have the `.part` suffix
(this is a feature of many existing clients) which is only removed once the
download finishes. The test script is going to continuously query for the
existence of the resulting file, and if it is found, it concludes the download
finished and compares the downloaded file with the file that was used for
seeding.


## Set up test environment

Instructions are given on how to set up the test environment and the required
data (found in [`assets`](assets)).

#### Generate test file
1. `cd assets`
2. Truncate the file to the desired size:
  ```bash
  truncate -s 1M 1mb-test.txt
  ```
3. Use python 3 to fill it with random printable characters:
  ```python
  import random, string
  with open('1mb-test.txt', 'w') as f:
      f.write(''.join(random.choices(string.ascii_lowercase + string.ascii_uppercase + string.digits, k=1048576)))
  ```

#### Create torrent env
4. Spawn Transmission docker container:
  ```bash
  docker run --rm \
    --name transmission \
    -p 9091:9091 \
    -e PUID=$UID \
    -e PGID=$UID \
    --mount type=bind,src=$(pwd)/transmission/downloads,dst=/downloads \
    --mount type=bind,src=$(pwd)/transmission/watch,dst=/watch \
    --mount type=bind,src=$(pwd)/transmission/config,dst=/config \
    linuxserver/transmission
  ```
5. After running the Transmission container for the first time, it will set up
   the expected directory structure in the `assets/transmission`
   subdirectories.

#### Create torrent file to seed
6. In another terminal, move test file into the folder that is bound into the
   transmission container (so that the process inside it can see the file):
  ```bash
  mv 1mb-test.txt transmission/downloads/complete
  ```
7. Create symbolic link for the file so that it's also in the assets directory
   (as a convenience for our test runner so that we don't have to hard-code the
   Transmission folder structure in the tests):
  ```bash
  ln -s transmission/downloads/complete/1mb-test.txt 1mb-test.txt
  ```
8. Create a shell session for the running Transmission container:
  ```bash
  docker exec -ti transmission bash
  ```
9. Inside the container, create the torrent metainfo file. **NOTE**: the file is
   not created not with the `.torrent` suffix on purpose! This is so that the
   Transmission daemon doesn't pick it up, as we have a root session in
   container and thus the file created will be owned by root, but the docker
   container was specified to run the daemon with the `UID` and `GID` of the host
   user (most likely not root), so the process would fail to read in the torrent
   file.
  ```bash
  transmission-create -o /watch/1mb-test.txt /downloads/complete/1mb-test.txt
  ```
10. In another terminal on the host, change ownership of the torrent file to the
   same user whose `UID` and `GID` were given to the Transmission container as
   env vars:
  ```bash
  sudo chown $USER:$USER transmission/watch/1mb-test.txt
  ```
11. Rename the torrent file to have the `.torrent` suffix, which will make the
    Transmission daemon automatically start seeding the torrent:
  ```bash
  mv transmission/watch/1mb-test.txt{,.torrent}
  ```
12. Go to the web UI at `localhost:9091` to verify that the torrent file is being
   seeded.
13. You can kill the Transmission container now by pressing CTRL+C in its
    terminal session as resume files are saved in the
    [`transmission/config/torrents`](`assets/transmission/config/torrents`)
    directory, which means the next time you run the container, it will continue
    these torrents. This fact is used to make reproducible downloads.
