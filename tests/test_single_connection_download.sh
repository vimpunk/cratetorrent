#!/bin/bash

# This test sets up a single transmission seeder and a cratetorrent leecher and
# asserts that cratetorrent downloads a single 1 MiB file from the seed
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
# this is the default Transmission port
seed_port=51413
seed_addr="${seed_ip}:${seed_port}"
seed_container=tr-seed-1

# start the container (if it's not already running)
./start_transmission_seed.sh --name "${seed_container}" --ip "${seed_ip}"

assets_dir="$(pwd)/assets"
torrent_name=1mb-test.txt
# the seeded file
source_path="${assets_dir}/${torrent_name}"
# and its metainfo
metainfo_path="${source_path}.torrent"
metainfo_cont_path="/cratetorrent/${torrent_name}.torrent"

# start seeding the torrent, if it doesn't exist yet
if [ ! -f "${source_path}" ]; then
    echo "Starting seeding of torrent ${torrent_name} seeding"
    torrent_size=$(( 1024 * 1024 )) # 1 MiB
    # first, we need to generate a random file
    ./create_random_file.sh --path "${source_path}" --size "${torrent_size}"
    # then start seeding it
    ./seed_new_torrent.sh \
        --name "${torrent_name}" \
        --path "${source_path}" \
        --seed "${seed_container}"
fi

# sanity check that after starting the seeding, the source files
# were properly generated
if [ ! -f "${metainfo_path}" ]; then
    echo "Error: metainfo ${metainfo_path} does not exist!"
    exit "${metainfo_not_found}"
fi
if [ ! -f "${source_path}" ]; then
    echo "Error: source file ${source_path} does not exist!"
    exit "${source_not_found}"
fi

# where we download our file on the host and in the container
download_dir=/tmp/cratetorrent

# initialize download directory to state expected by the cratetortent-cli
if [ -d "${download_dir}" ]; then
    echo "Clearing download directory ${download_dir}"
    rm -rf "${download_dir}"/*
elif [ -f "${download_dir}" ]; then
    echo "Error: file found where download directory ${download_dir} is supposed to be"
    exit "${dest_in_use}"
elif [ ! -d "${download_dir}" ]; then
    echo "Creating download directory ${download_dir}"
    mkdir -p "${download_dir}"
fi

# provide way to override the log level but default to tracing everything in the
# cratetorrent lib and binary
rust_log=${RUST_LOG:-cratetorrent=trace,cratetorrent_cli=trace}

# start cratetorrent leech container, which will run till the torrent is
# downloaded or an error occurs
time docker run \
    -ti \
    --rm \
    --env SEED="${seed_addr}" \
    --env METAINFO_PATH="${metainfo_cont_path}" \
    --env DOWNLOAD_DIR="${download_dir}" \
    --env RUST_LOG="${rust_log}" \
    --mount type=bind,src="${metainfo_path}",dst="${metainfo_cont_path}" \
    --mount type=bind,src="${download_dir}",dst="${download_dir}" \
    cratetorrent-cli

# the final download destination on the host
download_path="${download_dir}/${torrent_name}"

# first check if the file was downloaded in the expected path
if [ ! -f "${download_path}" ]; then
    echo "FAILURE: downloaded file ${download_path} does not exist!"
    exit "${download_not_found}"
fi

# assert that the downloaded file is the same as the original
echo
echo "Comparing downloaded file ${download_path} to source file ${source_path}"
if cmp --silent "${download_path}" "${source_path}"; then
    echo "SUCCESS: downloaded file matches source file"
    exit 0
else
    echo "FAILURE: downloaded file does not match source file"
    exit "${invalid_download}"
fi
