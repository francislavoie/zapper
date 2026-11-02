# A sample image for the static zapper-server for Linux (packaging/server/README.md, "In a
# container"). No image is published: build your own, from a folder holding this file and the
# release's zapper-server-<version>-linux-x86_64 renamed zapper-server:
#
#   docker build -t zapper-server .
#   docker run -d --name zapper-server -p 8796:8796 --stop-timeout 150 -e TZ=Europe/Paris \
#       -v zapper-data:/var/lib/zapper zapper-server
#
# On distroless's static image: the certificate authorities (providers and guides over HTTPS)
# and the time zones (TZ) are there, and nothing else, not even a shell. Everything it keeps is
# in the /var/lib/zapper volume (ZAPPER_HOME): settings, the provider account, the catalogue,
# and recordings in data/recordings unless another folder is chosen. It runs as the image's
# non-root user, uid 65532: a folder mounted for recordings must be writable by it.

# The volume's folder, made and handed to that user here: the static image has no mkdir.
FROM alpine:3.23 AS home
RUN mkdir -p /var/lib/zapper && chown 65532:65532 /var/lib/zapper

FROM gcr.io/distroless/static-debian12:nonroot
COPY --from=home --chown=65532:65532 /var/lib/zapper /var/lib/zapper
COPY --chmod=0755 zapper-server /usr/local/bin/zapper-server
ENV ZAPPER_HOME=/var/lib/zapper
VOLUME /var/lib/zapper
EXPOSE 8796
# SIGTERM (docker stop) finishes what it is recording into playable files before it exits:
# give it time with --stop-timeout.
ENTRYPOINT ["/usr/local/bin/zapper-server"]
