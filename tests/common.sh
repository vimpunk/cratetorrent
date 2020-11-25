#!/bin/bash

# error codes
metainfo_not_found=1
source_not_found=2
dest_in_use=3
download_not_found=4
invalid_download=5

# this is the default Transmission port
transmission_port=51413
seed_container=tr-seed-1
seed2_container=tr-seed-2

assets_dir="$(pwd)/assets"

# Verifies that the downloaded file or directory is the same as its source.
#
# Arguments:
# - $1 source torrent absolute path
# - $2 downloaded torrent absolute path
function verify_download {
    src=$1
    dl=$2
    if [ ! -e "${dl}" ]; then
        echo "FAILURE: destination file ${dl} does not exist!"
        exit "${download_not_found}"
    fi

    # assert that the downloaded file is the same as the original
    echo
    echo "Comparing downloaded torrent ${dl} to source ${src}"
    if ! diff -q -r "${dl}" "${src}"; then
        echo "FAILURE: downloaded torrent does not match source file"
        exit "${invalid_download}"
    fi
}

# Returns the container's IP address in the local Docker network.
#
# Arguments:
# - $1 container name
function get_container_ip {
    cont=$1
    docker inspect -f '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' "${cont}"
}
