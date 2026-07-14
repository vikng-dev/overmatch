FROM ubuntu:24.04@sha256:4fbb8e6a8395de5a7550b33509421a2bafbc0aab6c06ba2cef9ebffbc7092d90

RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
        bash \
        coreutils \
        libasound2t64 \
        libudev1 \
        libwayland-client0 \
        libx11-6 \
        libxkbcommon0 \
        time \
    && rm -rf /var/lib/apt/lists/*

COPY run-test-with-memory.sh /usr/local/bin/run-test-with-memory
RUN chmod 0755 /usr/local/bin/run-test-with-memory

ENTRYPOINT ["/usr/local/bin/run-test-with-memory"]
