#!/bin/bash

# This test sets up a single transmission seeder and a cratetorrent leecher and
# asserts that cratetorrent downloads a single 1MiB file from the seed
# correctly.

set -e

# the default docker bridge network is on subnet 172.17.0.0/16, the gateway on
# 172.17.0.1, so we can pin our seed to an IP in that subnet
#
# TODO: Is the subnet IP of a docker bridge network dynamically generated? Maybe
# it would be safer to prase the subnet mask or even create our own user-defined
# bridge network.
seed_ip=172.17.0.2
seed_port=51413
seed_addr="${seed_ip}:${seed_port}"
seed_cont_name=transmission

metainfo_path="$(pwd)/assets/1mb-test.txt.torrent"
metainfo_cont_path=/cratetorrent/1mb-test.txt.torrent

if [ ! -f "${metainfo_path}" ]; then
    echo "Metainfo at ${metainfo_path} not found"
    exit 1
fi

# start seed container if it isn't running
if ! docker inspect --format '{{.State.Running}}' "${seed_cont_name}" > /dev/null; then
    echo "Starting Transmission seed container"
    docker run \
        --rm \
        --name "${seed_cont_name}" \
        --publish 9091:9091 \
        --env PUID=$UID \
        --env PGID=$UID \
        --mount type=bind,src="$(pwd)/assets/transmission/downloads",dst=/downloads \
        --mount type=bind,src="$(pwd)/assets/transmission/watch",dst=/watch \
        --mount type=bind,src="$(pwd)/assets/transmission/config",dst=/config \
        --ip "${seed_ip}" \
        --detach \
        linuxserver/transmission

    # wait for seed to come online
    sleep 5
fi

# start cratetorrent leech container
docker run \
    -ti \
    --env SEED="${seed_addr}" \
    --env METAINFO_PATH="${metainfo_cont_path}" \
    --env RUST_LOG=trace,cratetorrent=trace,cratetorrent_cli=trace \
    --mount type=bind,src="${metainfo_path}",dst="${metainfo_cont_path}" \
    cratetorrent-cli

# TODO

# wait until file is downloaded

# assert that the downloaded file is the same as the original
