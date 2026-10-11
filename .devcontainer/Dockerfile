FROM rust:1.99-trixie

RUN apt-get update && export DEBIAN_FRONTEND=noninteractive \
    && apt-get -y install --no-install-recommends \
    build-essential ca-certificates locales man-db \
    htop procps psmisc telnet dnsutils net-tools iputils-ping \
    wget curl zip unzip git less jq ripgrep vim-nox rsync openssh-client netcat-traditional
