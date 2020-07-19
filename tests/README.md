# Integration tests

Each integration test case tests a different type of expected functionality of
cratetorrent.

In general all tests use Docker containers and a Docker virtual network to
simulate a primitive network of peers (later on this may be expanded so that
peers are not on the same LAN), and one or more of the containers will use the
established
[Transmission](https://manpages.ubuntu.com/manpages/bionic/man1/transmission-cli.1.html)
torrent client, against which to test cratetorrent's protocol compliance, i.e.
to make sure that cratetorrent works with torrent clients used in the wild.

To ensure reproducible test results, it is always the same file that is going to
be downloaded or seeded, and external clients are not going to be involved, and
the client is going to connect to the seed directly, without the involvement of
BitTorrent trackers or the DHT, that may introduce variable test runs. These are
supposed to be tested separately at a later time point, when functionality is
added.

Moreover, various file sizes are going to be tested to ensure that cratetorrent
works correctly with small and large files, with different piece sizes and other
parameters.

A successful test run should look like the following:
```
[2020-07-19T11:59:22Z INFO  cratetorrent::torrent] Finished torrent download, exiting
[2020-07-19T11:59:22Z TRACE cratetorrent::storage_info] Returning files intersecting piece 31
[2020-07-19T11:59:22Z TRACE cratetorrent::disk] Shutting down disk IO task
[2020-07-19T11:59:22Z DEBUG cratetorrent::disk::io] Piece 31 intersects files: 0..1
[2020-07-19T11:59:22Z INFO  cratetorrent::peer] Shutting down peer 172.17.0.2:51413 session
[2020-07-19T11:59:22Z DEBUG cratetorrent::disk::io] Disk received command
[2020-07-19T11:59:22Z TRACE cratetorrent::disk::io] Saving torrent 0 block BlockInfo { piece_index: 31, offset: 16384, len: 16384 } to disk
[2020-07-19T11:59:22Z TRACE cratetorrent::disk::io] Saving block BlockInfo { piece_index: 31, offset: 16384, len: 16384 } to disk
[2020-07-19T11:59:22Z DEBUG cratetorrent::disk::io] Piece hash: 8048e7746ba19b1e52ea6396bfcc8d5240b021f3
[2020-07-19T11:59:22Z INFO  cratetorrent::disk::io] Piece 31 is valid
[2020-07-19T11:59:22Z DEBUG cratetorrent::disk::io] Disk received command
[2020-07-19T11:59:22Z INFO  cratetorrent::disk::io] Shutting down disk event loop

real	0m14,614s
user	0m0,036s
sys	0m0,029s

Comparing downloaded file /tmp/cratetorrent/1mb-test.txt to source file ~/code/cratetorrent/tests/assets/1mb-test.txt
SUCCESS: downloaded file matches source file
```

## Prerequisites

To run the tests, you first first need to build the `cratetorrent-cli` binary
and its corresponding docker image. For instructions, see the [project
readme](../README.md).


## Environment

Each test case sets up its own environment (e.g. a seed container and files to
seed), using the [`start_transmission_seed.sh`](./start_transmission_seed.sh)
and [`seed_new_torrent.sh`](./seed_new_torrent.sh) utility scripts. After the
local environment is set up, tests will be able to reuse their environment,
meaning they need not be generated again (which is on purpose, so that one can
perform testing against the same seed(s) and file(s) repeatedly). However, the
generated files are not tracked in version control to avoid bloat, so these are
only "constant" for local development. These files will be placed in the
`assets` directory (created by the scripts).

**Hint**: run any script with the `--help` flag for more info.

### Test binary

cratetorrent-cli arguments:
- the seed's address in the local Docker network,
- and the download destination directory.

It runs only as long as the download is in progress. Once it's done, it exits,
and this fact is used by the test scripts to perform download verification
afterwards. Later this will change to something more sophisticated, like
detecting when a download is no longer a partial download (e.g. with the `.part`
suffix).


## Test scenarios

### Single small file download

- **Goal**: the successful download of a single (small) file, asserting basic
  correctness
- **Command**:
[`./test_single_connection_download.sh`](test_single_connection_download.sh)
- **Containers**:
  - *seeds:*
    - tranasmission
  - *leeches:*
    - cratetorrent-cli
- **File**: 1 MiB file at `assets/1mb-test.txt`


## Set up test environment

Instructions are given on how to manually set up the test environment and the
required data. This is for documentation purposes, prefer the
[`start_transmission_seed.sh`](./start_transmission_seed.sh) and
[`seed_new_torrent.sh`](./seed_new_torrent.sh) scripts.

#### Generate test file
1. Create the necessary directories, `assets`,
   `assets/transmission/{downloads,watch,config}`, and `cd assets`.
2. Truncate the file to the desired size:
  ```bash
  truncate -s 1M 1mb-test.txt
  ```
3. Fill file with random characters
  ```bash
  head -c 1048576 < /dev/urandom > 1mb-test.txt
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
9. Inside the container, create the torrent metainfo file. **NOTE**: The file is
   not created with the `.torrent` suffix on purpose! Since the Transmission
   container can only be run as root, the metainfo file is also created as root.
   However, this would cause a permission denied error for the transmission
   container itself, as it is running as the specified user/group.
   By not specifying the `.torrent` suffix, Transmission won't pick it up, so we
   get a chance to change its permissions before adding the suffix. 
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
