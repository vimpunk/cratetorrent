#!/bin/bash

# This file sets up a test environment with specific details supplied as command
# line arguments.
#
# It creates a metainfo of the given torrent file or directory and starts
# seeding it in the given Transmission container.

set -e

function print_help {
    echo -e "
This makes the seed container start seeding the given file (generating its necessary metainfo).
The seed container must be running.

USAGE: $1 --path <path> [--seed <seed>] 

OPTIONS:
    -n|--name       The name of the torrent (currently the name of the file).
    -p|--path       The path of the torrent file or directory to seed. This will
                    be copied into the container's downloads complete folder.
    --seed          The Transmission seed container's name. Must be running.
                    Defaults to 'transmission'.
    -h|--help       Print this help message.
    "
}

for arg in "$@"; do
    case "${arg}" in
        -n|--name)
            torrent_name=$2
        ;;
        -p|--path)
            path=$2
        ;;
        --seed)
            seed_container=$2
        ;;
        --h|--help)
            print_help
            exit 0
        ;;
    esac
    shift
done

if [ -z "${torrent_name}" ]; then
    echo "Error: --name must be set"
    print_help
    exit 1
fi

if [ -z "${path}" ]; then
    echo "Error: --path must be set"
    print_help
    exit 1
fi

if [ -z "${seed_container}" ]; then
    echo "Error: --seed must be set"
    print_help
    exit 1
fi


################################################################################
# 1. Verify seed
################################################################################

# where the torrent file and metainfo are saved
assets_dir="$(pwd)/assets"
if [ ! -d "${assets_dir}" ]; then
    echo "Error: assets directory ${assets_dir} does not exist"
    exit 2
fi

# check that the container's directories exist
tr_seed_dir="${assets_dir}/${seed_container}"
if [ ! -d "${tr_seed_dir}" ]; then
    echo "Error: seed directory ${tr_seed_dir} does not exist"
    exit 3
fi
tr_config_dir="${tr_seed_dir}/config"
tr_downloads_dir="${tr_seed_dir}/downloads"
tr_watch_dir="${tr_seed_dir}/watch"
for subdir in {"${tr_config_dir}","${tr_downloads_dir}","${tr_watch_dir}"}; do
    if [ ! -d "${subdir}" ]; then
        echo "Error: seed subdirectory ${subdir} does not exist"
        exit 4
    fi
done

# check if the seed is running: if not, abort
if ! docker inspect --format '{{.State.Running}}' "${seed_container}" &> /dev/null
then
    echo "Error: Transmission seed container ${seed_container} is not running"
    exit 5
fi

################################################################################
# 2. Verify file to seed
################################################################################

if [ ! -f "${path}" ] && [ ! -d "${path}" ]; then
    echo "Error: torrent file does not exist at ${path}"
    exit 6
fi

################################################################################
# 3. Create torrent
################################################################################

# The source to be seeded must be inside the container. For this reason, if it
# is not already there, we copy it in the torrent downloads complete folder.
# This should be a noop even for large files as we're not actually modifying the
# source so linux should do a copy-on-write here.
torrent_path="${tr_downloads_dir}/complete/${torrent_name}"
if [ "${path}" != "${torrent_path}" ]; then
    echo "Copying torrent from source ${path} to seed dir at ${torrent_path}"
    cp -r "${path}" "${torrent_path}"
fi

# create the torrent inside the seed container
#
# NOTE: The file is not created with the `.torrent` suffix on purpose! Since the
# Transmission container can only be run as root, the metainfo file is also
# created as root.  However, this would cause a permission denied error for the
# Transmission container itself, as it is running as the specified user/group.
# By not specifying the `.torrent` suffix, Transmission won't pick it up, so we
# get a chance to change its permissions before adding the suffix. 
#
# It may be possible to solve this by spawning a subshell with a different EUID
# and execute the `transmission-create` command there, but currently it is not
# clear how to do this.

echo "Creating torrent metainfo file"
docker exec "${seed_container}" transmission-create \
  -o "/watch/${torrent_name}" \
  "/downloads/complete/${torrent_name}"

# change ownership of the metainfo file to the same user whose `UID` and `GID`
# were given to the seed container
#
# TODO: make this work without sudo
echo "Changing metainfo owner from root to $USER"
metainfo_path="${tr_watch_dir}/${torrent_name}"
sudo chown $USER:$USER "${metainfo_path}"

# rename the torrent file to have the `.torrent` suffix, which will make the
# Transmission daemon automatically start seeding the torrent
echo "Adding .torrent suffix to metainfo filename"
mv "${metainfo_path}" "${metainfo_path}.torrent"
# wait for Transmission to pick up the file
sleep 5
# we need to add the `.added` suffix to our path as that's what Transmission does after
# picking up a new metainfo file
metainfo_path="${metainfo_path}.torrent.added"
# sanity check
if [ ! -f "${metainfo_path}" ]; then
    echo "Error: could not find metainfo ${metainfo_path} after starting torrent"
    exit 7
fi

# link metainfo file in the transmission watch directory to the root of the
# assets dir for the convenience of other scripts
echo "Linking metainfo to assets directory root"
ln -s "${metainfo_path}" "${assets_dir}/${torrent_name}.torrent"

echo "Done!"
echo "Torrent ${torrent_name} is seeded by container ${seed_container}"
