#!/bin/bash

# This test sets up two transmission seeders and a cratetorrent leecher and
# asserts that cratetorrent downloads a single 1 MiB file from both seeds
# correctly.

set -e

source common.sh


function print_help {
    echo -e "
USAGE: $1 [ --size <size[unit]> ]

OPTIONS:
    -s|--size       The size of the torrent file, in bytes.
    -h|--help       Print this help message.
    "
}

for arg in "$@"; do
    case "${arg}" in
        --size)
            torrent_size=$2
        ;;
        --h|--help)
            print_help
            exit 0
        ;;
    esac
    shift
done


# start the seeds (if not already running)
./start_transmission_seed.sh --name "${seed_container}"
./start_transmission_seed.sh --name "${seed2_container}"

# default torrent size to 1 MiB
torrent_size="${torrent_size:-$(( 1024 * 1024 ))}"
torrent_name="muti-peer-test-${torrent_size}"
# the seeded file
src_path="${assets_dir}/${torrent_name}"
# and its metainfo
metainfo_path="${src_path}.torrent"
metainfo_cont_path="/cratetorrent/${torrent_name}.torrent"
# where we download the torrent (the same path is used on both the host and in
# the container)
download_dir=/tmp/cratetorrent
# the final download destination on the host
download_path="${download_dir}/${torrent_name}"

################################################################################
# 1. Env setup
################################################################################

# start seeding the torrent, if it doesn't exist yet
if [ ! -f "${src_path}" ]; then
    echo "Starting seeding of torrent ${torrent_name}"
    # first, we need to generate a random file
    ./create_random_file.sh --path "${src_path}" --size "${torrent_size}"
    # then start seeding it by both torrents
    ./seed_new_torrent.sh \
        --name "${torrent_name}" \
        --path "${src_path}" \
        --seed "${seed_container}"
    ./seed_new_torrent.sh \
        --name "${torrent_name}" \
        --path "${src_path}" \
        --seed "${seed2_container}"
fi

################################################################################
# 2. Download
################################################################################

./test_download.sh --torrent-name "${torrent_name}" \
    --src-path "${src_path}" \
    --download-dir "${download_dir}" \
    --metainfo-path "${metainfo_path}" \
    --seeds "${seed_container},${seed2_container}"

################################################################################
# 3. Verification
################################################################################

# assert that the downloaded file is the same as the original
verify_file "${src_path}" "${download_path}"

echo
echo "SUCCESS: downloaded file matches source file"
