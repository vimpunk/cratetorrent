#!/bin/bash

# This test sets up a single transmission seeder and a cratetorrent leecher and
# asserts that cratetorrent downloads a single 1MiB file from the seed
# correctly.


set -e

# error codes
metainfo_not_found=1
source_not_found=2
dest_in_use=3
download_not_found=4
invalid_download=5

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

# provide way to override the log level but default to tracing everything in the
# cratetorrent lib and binary
RUST_LOG=${RUST_LOG:-cratetorrent=trace,cratetorrent_cli=trace}

metainfo_path="$(pwd)/assets/1mb-test.txt.torrent"
metainfo_cont_path=/cratetorrent/1mb-test.txt.torrent
if [ ! -f "${metainfo_path}" ]; then
    echo "Metainfo at ${metainfo_path} not found"
    exit "${metainfo_not_found}"
fi

download_dir=/tmp/cratetorrent
download_cont_dir=/tmp

# the final download destination
download_path="${download_dir}/1mb-test.txt"
# the source file's path
source_path=assets/1mb-test.txt
# sanity check
if [ ! -f "${source_path}" ]; then
    echo "Error: source file ${source_path} does not exist!"
    exit "${source_not_found}"
fi

# initialize download directory to state expcted by the cratetortent-cli
if [ -d "${download_dir}" ]; then
    echo "Clearing download directory ${download_dir}"
    rm -rf "${download_dir}"/*
elif [ ! -d "${download_dir}" ]; then
    echo "Creating download directory ${download_dir}"
    mkdir -p "${download_dir}"
elif [ -f "${download_dir}" ]; then
    echo "File found where download directory ${download_dir} is supposed to be"
    exit "${dest_in_use}"
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

# start cratetorrent leech container, which will run until the torrent is not
# downloaded or an error occurs
time docker run \
    -ti \
    --env SEED="${seed_addr}" \
    --env METAINFO_PATH="${metainfo_cont_path}" \
    --env DOWNLOAD_DIR="${download_cont_dir}" \
    --env RUST_LOG="${RUST_LOG}" \
    --mount type=bind,src="${metainfo_path}",dst="${metainfo_cont_path}" \
    --mount type=bind,src="${download_dir}",dst="${download_cont_dir}" \
    cratetorrent-cli

# first check if the file was downloaded in the expected path
if [ ! -f "${download_path}" ]; then
    echo "Error: downloaded file ${download_path} does not exist!"
    exit "${download_not_found}"
fi

# assert that the downloaded file is the same as the original
echo
echo "Comparing downloaded file ${download_path} to source file ${source_path}"
if cmp --silent "${download_path}" "${source_path}"; then
    echo "Success! Downloaded file matches source file"
    exit 0
else
    echo "Failure: downloaded file does not match source file"
    exit "${invalid_download}"
fi
