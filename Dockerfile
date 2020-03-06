FROM ubuntu:18.04

WORKDIR /cratetorrent

COPY /target/release/cratetorrent-cli .

CMD ./cratetorrent-cli --seed "${SEED}" --metainfo "${METAINFO_PATH}"
