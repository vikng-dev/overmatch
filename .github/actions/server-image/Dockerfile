# The hosted server image: the `overmatch-server` tarball's tree on the Ubuntu release the binary
# was linked on (BASE, chosen by action.yml), plus libudev for Bevy's gamepad dependency.
# No CA store: ureq's rustls carries its own webpki roots (Cargo.toml). No asset-root env: the tree
# is flat, so `crate::assets::asset_root` and Bevy's reader both resolve `assets/` beside the binary.
ARG BASE
FROM ${BASE}
RUN apt-get update \
 && apt-get install -y --no-install-recommends libudev1 \
 && rm -rf /var/lib/apt/lists/*
COPY overmatch-server /opt/overmatch-server
WORKDIR /opt/overmatch-server
EXPOSE 5888/udp
ENTRYPOINT ["/opt/overmatch-server/overmatch-server"]
