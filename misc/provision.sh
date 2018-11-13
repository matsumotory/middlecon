#/bin/sh

sudo apt-get update
sudo apt-get -y install build-essential rake bison git gperf automake m4 \
                autoconf libtool cmake pkg-config libcunit1-dev ragel

curl https://sh.rustup.rs -sSf > rustup.sh
sh rustup.sh -y
source $HOME/.cargo/env
rustup update
rustup install nightly
rustup component add rustfmt-preview --toolchain nightly
rustup default nightly

git clone https://github.com/matsumotory/middlecon
cd middlecon && cargo build
