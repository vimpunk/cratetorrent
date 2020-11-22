#!/bin/bash

set -e

source common.sh

function print_help {
    echo -e "
This is a helper script for taking care of the common tasks of testing downloads.

USAGE: $1 --torrent-name NAME \\
          --src-path PATH \\
          --download-dir PATH \\
          --metainfo-dir PATH \\
          --listen-addr ADDR \\
          --seeds SEED1,SEED2,...

OPTIONS:
    -h|--help   Print this help message.
    "
}

for arg in "$@"; do
    case "${arg}" in
        --torrent-name)
            torrent_name=$2
        ;;
        --src-path)
            src_path=$2
        ;;
        --download-dir)
            download_dir=$2
        ;;
        --metainfo-path)
            metainfo_path=$2
        ;;
        --listen-addr)
            listen_addr=$2
        ;;
        --seeds)
            IFS=',' read -r -a seeds <<< "$2"
        ;;
        --seed-port)
            seed_port=$2
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
if [ -z "${src_path}" ]; then
    echo "Error: --src-path must be set"
    print_help
    exit 1
fi
if [ -z "${download_dir}" ]; then
    echo "Error: --download-dir must be set"
    print_help
    exit 1
fi
if [ -z "${metainfo_path}" ]; then
    echo "Error: --metainfo-path must be set"
    print_help
    exit 1
fi
if [ -z "${listen_addr}" ]; then
    echo "Error: --listen-addr must be set"
    print_help
    exit 1
fi
if [ -z "${seeds}" ]; then
    echo "Error: --seeds must be set"
    print_help
    exit 1
fi

seed_port="${seed_port:-${transmission_port}}"

# collect the IP addresses of all seeds within the docker network
for seed in "${seeds[@]}"; do
    ip="$(get_container_ip "${seed}")"
    addr="${ip}:${seed_port}"
    if [ -z "${seeds_addrs}" ]; then
        seeds_addrs="${addr}"
    else
        seeds_addrs="${seeds_addrs},${addr}"
    fi
done

echo "Seeds: ${seeds[@]}"
echo "Addresses of seeds: ${seeds_addrs}"

################################################################################
# 1. Source files existence check
################################################################################

# sanity check that after starting the seeding, the source files
# were properly generated
if [ ! -f "${metainfo_path}" ]; then
    echo "Error: metainfo ${metainfo_path} does not exist!"
    exit "${metainfo_not_found}"
fi
if [ ! -e "${src_path}" ]; then
    echo "Error: source file ${src_path} does not exist!"
    exit "${source_not_found}"
fi

################################################################################
# 2. Download
################################################################################

# where we download the torrent (the same path is used on both the host and in
# the container)
download_dir=/tmp/cratetorrent

# initialize download directory to state expected by the cratetorrent-cli
if [ -d "${download_dir}" ]; then
    echo "Clearing download directory ${download_dir}"
    sudo rm -rf "${download_dir}"/*
elif [ -f "${download_dir}" ]; then
    echo "Error: file found where download directory ${download_dir} is supposed to be"
    exit "${dest_in_use}"
elif [ ! -d "${download_dir}" ]; then
    echo "Creating download directory ${download_dir}"
    mkdir -p "${download_dir}"
fi

# provide way to override the log level but default to tracing everything in the
# cratetorrent lib and binary
rust_log=${RUST_LOG:-warn,cratetorrent=trace,cratetorrent_cli=trace}
# the path of the metainfo inside the container
metainfo_cont_path="/cratetorrent/${torrent_name}.torrent"

# start cratetorrent leech container, which will run till the torrent is
# downloaded or an error occurs
time docker run \
    -ti \
    --rm \
    --env LISTEN="${listen_addr}" \
    --env SEEDS="${seeds_addrs}" \
    --env METAINFO_PATH="${metainfo_cont_path}" \
    --env DOWNLOAD_DIR="${download_dir}" \
    --env RUST_LOG="${rust_log}" \
    --mount type=bind,src="${metainfo_path}",dst="${metainfo_cont_path}" \
    --mount type=bind,src="${download_dir}",dst="${download_dir}" \
    cratetorrent-cli

################################################################################
# 3. Destination existence check
################################################################################

# the final download destination on the host
download_path="${download_dir}/${torrent_name}"
# assert that the downloaded file is the same as the original
verify_download "${src_path}" "${download_path}"

echo
echo "SUCCESS: downloaded file matches source file"
