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

# Verifies that the downloaded file is the same as its source file.
#
# Arguments:
# - $1 source file absolute path
# - $2 downloaded file absolute path
function verify_file {
    src=$1
    dl=$2
    if [ ! -f "${dl}" ]; then
        echo "FAILURE: destination file ${dl} does not exist!"
        exit "${download_not_found}"
    fi

    # assert that the downloaded file is the same as the original
    echo
    echo "Comparing downloaded file ${dl} to source file ${src}"
    if ! cmp --silent "${dl}" "${src}"; then
        echo "FAILURE: downloaded file does not match source file"
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
