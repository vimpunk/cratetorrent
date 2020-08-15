#!/bin/bash

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
# it would be safer to parse the subnet mask or even create our own user-defined
# bridge network.
seed_ip=172.17.0.2
# this is the default Transmission port
seed_port=51413
seed_addr="${seed_ip}:${seed_port}"
seed_container=tr-seed-1

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
