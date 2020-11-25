#!/bin/bash

set -e

source common.sh

function print_help {
    echo -e "
This script starts a Transmission seed container with the specified parameters,
if it's not already running.

USAGE: $1 --name NAME

OPTIONS:
    -n|--name   The name to give to the container.
    -h|--help   Print this help message.
    "
}

for arg in "$@"; do
    case "${arg}" in
        -n|--name)
            name=$2
        ;;
        --h|--help)
            print_help
            exit 0
        ;;
    esac
    shift
done

if [ -z "${name}" ]; then
    echo "Error: --name must be set"
    print_help
    exit 1
fi

# where the torrent file and metainfo are saved
assets_dir="$(pwd)/assets"
if [ -f "${assets_dir}" ]; then
    echo "Error: file found at assets directory path ${assets_dir} "
    exit 2
elif [ ! -d "${assets_dir}" ]; then
    echo "Creating assets directory ${assets_dir}"
    mkdir "${assets_dir}"
fi

# initialize the directories of the seed, if needed
#
# NOTE: the paths given are on the host, not inside the container
tr_seed_dir="${assets_dir}/${name}"
if [ ! -d "${tr_seed_dir}" ]; then
    echo "Creating seed ${name} directory at ${tr_seed_dir}"
    mkdir "${tr_seed_dir}"
fi
tr_config_dir="${tr_seed_dir}/config"
tr_downloads_dir="${tr_seed_dir}/downloads"
tr_watch_dir="${tr_seed_dir}/watch"
# create the subdirectories that we're binding into the container (must exist
# before bind mounting)
for subdir in {"${tr_config_dir}","${tr_downloads_dir}","${tr_watch_dir}"}; do
    if [ ! -d "${subdir}" ]; then
        echo "Creating seed subdirectory ${subdir}"
        mkdir "${subdir}"
    fi
done

# check if the seed is running: if not, start it
if ! docker inspect --format '{{.State.Running}}' "${name}" &> /dev/null
then
    echo "Starting Transmission seed container ${name} listening on port 51413"
    docker run \
        --rm \
        --name "${name}" \
        --env PUID=$UID \
        --env PGID=$UID \
        --mount type=bind,src="${tr_config_dir}",dst=/config \
        --mount type=bind,src="${tr_downloads_dir}",dst=/downloads \
        --mount type=bind,src="${tr_watch_dir}",dst=/watch \
        --detach \
        linuxserver/transmission

    seed_ip="$(get_container_ip "${name}")"
    echo "Seed available on local Docker net at IP: ${seed_ip}"

    # wait for seed to come online
    sleep 5

    echo "Transmission seed ${name} started!"
else
    echo "Transmission seed ${name} already running!"
fi
