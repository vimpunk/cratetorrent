FROM ubuntu:18.04

WORKDIR /cratetorrent

COPY /target/release/cratetorrent-cli .

CMD ./cratetorrent-cli --seeds "${SEEDS}" --metainfo "${METAINFO_PATH}" --download-dir "${DOWNLOAD_DIR}"
