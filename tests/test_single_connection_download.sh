#!/bin/bash

# This test sets up a single transmission seeder and a cratetorrent leecher and
# asserts that cratetorrent downloads a single 1MiB file from the seed
# correctly.

# run seed container
docker run --rm \
    --name transmission \
    --publish 9091:9091 \
    --env PUID=$UID \
    --env PGID=$UID \
    --mount type=bind,src=assets/transmission/downloads,dst=/downloads \
    --mount type=bind,src=assets/transmission/watch,dst=/watch \
    --mount type=bind,src=assets/transmission/config,dst=/config \
    --detach \
    linuxserver/transmission

# wait for seed to come online
sleep 5

# TODO: run cratetorrent leech

# wait until file is downloaded

# assert that the downloaded file is the same as the original
