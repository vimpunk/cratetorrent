#!/bin/bash


function print_help {
    echo -e "
This script creates a file with the given size with random contents.

USAGE: $1 --path <path> --size <size[unit]>

OPTIONS:
    -p|--path       The path of the torrent file or directory.
    -s|--size       The size of the file, in bytes.
    -h|--help       Print this help message.
    "
}

for arg in "$@"; do
    case "${arg}" in
        -p|--path)
            path=$2
        ;;
        -s|--size)
            size=$2
        ;;
        --h|--help)
            print_help
            exit 0
        ;;
    esac
    shift
done

if [ -z "${path}" ]; then
    echo "Error: --path must be set"
    print_help
    exit 1
fi

if [ -z "${size}" ]; then
    echo "Error: --size must be set"
    print_help
    exit 1
fi

# make sure there is nothing at path
if [ -e "${path}" ]; then
    echo "Error: ${path} already exists"
    exit 2
fi

# truncate file
echo "Truncating file ${path} to ${size} bytes"
truncate -s "${size}" "${path}"

# use urandom to fill it with random characters

echo "Populating source file ${path} with random characters"
head -c "${size}" < /dev/urandom > "${path}"

echo "Done!"
echo "File randomly generated at ${path} with ${size} bytes"
