#!/bin/bash

set -ex

source common.sh

function print_help {
    echo -e "
This test sets up a single cratetorrent seeder and a cratetorrent leecher and
asserts that cratetorrent downloads a single 1 MiB file from the seed correctly,
which implies that cratetorrent seeding works as expected.

USAGE: $1 --torrent-name NAME

OPTIONS:
    --torrent-name  The name of the torrent, which has to exist in the
                    assets directory.
    -h|--help       Print this help message.
    "
}

for arg in "$@"; do
    case "${arg}" in
        --torrent-name)
            torrent_name=$2
        ;;
        --h|--help)
            print_help
            exit 0
        ;;
    esac
    shift
done

if [ -z "${torrent_name}" ]; then
    echo "Error: --torrent-name must be set"
    print_help
    exit 1
fi

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

# start seeding the torrent

rust_log=${RUST_LOG:-warn,cratetorrent=trace,cratetorrent_cli=trace}
seed_listen_port=51234
seed_listen_addr="0.0.0.0:${seed_listen_port}"
seed_cont_name="${torrent_name}-ct-seed"
src_dir="$(pwd)/assets/tr-seed-1/downloads/complete/"
src_cont_dir=/cratetorrent/torrents

# start cratetorrent leech container, which will run till the torrent is
# downloaded or an error occurs
time docker run \
    -ti \
    --rm \
    --name "${seed_cont_name}" \
    --env LISTEN="${seed_listen_addr}" \
    --env MODE=seed \
    --env METAINFO_PATH="${metainfo_cont_path}" \
    --env DOWNLOAD_DIR="${src_cont_dir}" \
    --env RUST_LOG="${rust_log}" \
    --mount type=bind,src="${metainfo_path}",dst="${metainfo_cont_path}" \
    --mount type=bind,src="${src_dir}",dst="${src_cont_dir}" \
    -d cratetorrent-test-cli

################################################################################
# 2. Download
################################################################################

leech_listen_addr=0.0.0.0:51888

./test_download.sh --torrent-name "${torrent_name}" \
    --src-path "${src_path}" \
    --download-dir "${download_dir}" \
    --metainfo-path "${metainfo_path}" \
    --listen-addr "${leech_listen_addr}" \
    --seeds "${seed_cont_name}" \
    --seed-port "${seed_listen_port}"

echo "Stopping seed container '${seed_cont_name}'"
# The download has completed successfully, remove seed. If it hasn't, it is not
# removed to allow debugging.
docker stop "${seed_cont_name}"
