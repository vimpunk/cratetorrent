FROM ubuntu:20.04

RUN apt-get -y update && \
    apt-get -y install libssl-dev openssl && \
    apt-get clean && \
    apt-get autoremove

WORKDIR /cratetorrent

COPY /target/release/test-cli .

# Note we're not quoting the arguments because e.g. in case of the seeds arg an
# empty string may be given and if quoted that would result in a `''`, which
# trips the cli arg parser.
CMD ./test-cli --listen ${LISTEN} --mode ${MODE} --seeds ${SEEDS} --metainfo ${METAINFO_PATH} --download-dir ${DOWNLOAD_DIR}
