FROM ubuntu:20.04

RUN apt-get -y update && \
    apt-get -y install libssl-dev openssl && \
    apt-get clean && \
    apt-get autoremove

WORKDIR /cratetorrent

COPY /target/release/cratetorrent-cli .

CMD ./cratetorrent-cli --listen "${LISTEN}" --mode "${MODE}" --seeds "${SEEDS}" --metainfo "${METAINFO_PATH}" --download-dir "${DOWNLOAD_DIR}" --quit-after-complete
