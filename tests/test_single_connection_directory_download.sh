#!/bin/bash

# This test sets up a single transmission seeder and a cratetorrent leecher and
# asserts that cratetorrent downloads a small archive of multiple files from the
# seed correctly.
#
# assets/dir-test
# |-file1.txt: 100 KiB 
# |-file2.txt: 17 KiB 
# |-subdir
#   |-file3.txt: 6923 b
#   |-subdir2
#     |-file4.txt: 234 b

set -e

source common.sh

# start the container (if it's not already running)
./start_transmission_seed.sh --name "${seed_container}" --ip "${seed_ip}"

torrent_name=dir-test
# the seeded file
src_path="${assets_dir}/${torrent_name}"
# and its metainfo
metainfo_path="${src_path}.torrent"
metainfo_cont_path="/cratetorrent/${torrent_name}.torrent"

# relative paths of the download resources
file1=file1.txt
file2=file2.txt
subdir1=subdir
file3="${subdir1}/file3.txt"
subdir2="${subdir1}/nested"
file4="${subdir2}/file4.txt"

################################################################################
# 1. Env setup
################################################################################

# top level
file1_path="${src_path}/${file1}"
file2_path="${src_path}/${file2}"
# first dir
subdir1_path="${src_path}/${subdir1}"
file3_path="${src_path}/${file3}"
# nested dir
subdir2_path="${src_path}/${subdir2}"
file4_path="${src_path}/${file4}"

# start seeding the torrent, if it doesn't exist yet
if [ ! -d "${src_path}" ]; then
    echo "Generating torrent ${torrent_name} directories and files"

    # create source directory and its subdirectories
    for dir in "${src_path}" "${subdir1_path}" "${subdir2_path}"; do
        if [ ! -d "${dir}" ]; then
            echo "Creating directory ${dir}"
            mkdir -p "${dir}"
        fi
    done

    # create source files
    file1_size=$(( 100 * 1024 )) # 100 KiB
    ./create_random_file.sh --path "${file1_path}" --size "${file1_size}"

    file2_size=$(( 17 * 1024 )) # 17 KiB
    ./create_random_file.sh --path "${file2_path}" --size "${file2_size}"

    file3_size=$(( 6923 )) # 6923 bytes
    ./create_random_file.sh --path "${file3_path}" --size "${file3_size}"

    file4_size=$(( 234 )) # 234 bytes
    ./create_random_file.sh --path "${file4_path}" --size "${file4_size}"

    # then start seeding it
    echo "Starting seeding of torrent ${torrent_name} seeding"
    ./seed_new_torrent.sh \
        --name "${torrent_name}" \
        --path "${src_path}" \
        --seed "${seed_container}"
fi

# sanity check that after starting the seeding, the source files
# were properly generated
if [ ! -f "${metainfo_path}" ]; then
    echo "Error: metainfo ${metainfo_path} does not exist!"
    exit "${metainfo_not_found}"
fi
# check directories first
for dir in "${src_path}" "${subdir1_path}" "${subdir2_path}"; do
    if [ ! -d "${dir}" ]; then
        echo "Error: source directory ${dir} does not exist!"
        exit "${source_not_found}"
    fi
done
# then the files
for file in "${file1_path}" "${file2_path}" "${file3_path}" "${file4_path}"; do
    if [ ! -f "${file}" ]; then
        echo "Error: source file ${file} does not exist!"
        exit "${source_not_found}"
    fi
done

################################################################################
# 2. Download
################################################################################

# where we download the torrent (the same path is used on both the host and in
# the container)
download_dir=/tmp/cratetorrent

# initialize download directory to state expected by the cratetortent-cli
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

################################################################################
# 3. Verification
################################################################################

# the final download destination paths on the host
download_path="${download_dir}/${torrent_name}"
# top level
download_file1="${download_path}/${file1}"
download_file2="${download_path}/${file2}"
# first dir
download_subdir1="${download_path}/${subdir1}"
download_file3="${download_path}/${file3}"
# nested dir
download_subdir2="${download_path}/${subdir2}"
download_file4="${download_path}/${file4}"

# check directories first
for dir in "${download_path}" "${download_subdir1}" "${download_subdir2}"
do
    if [ ! -d "${dir}" ]; then
        echo "FAILURE: destination directory ${dir} does not exist!"
        exit "${download_not_found}"
    fi
done

# then the files: existence and content equality
verify_file "${file1_path}" "${download_file1}"
verify_file "${file2_path}" "${download_file2}"
verify_file "${file3_path}" "${download_file3}"
verify_file "${file4_path}" "${download_file4}"

echo
echo "SUCCESS: downloaded archive matches source archive"
