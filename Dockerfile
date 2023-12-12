# thanks https://whitfin.io/speeding-up-rust-docker-builds/

# select build image
FROM rust:bookworm as build

# create a new empty shell project
RUN USER=root cargo new --bin bottle
WORKDIR /bottle

# copy over your manifests
COPY ./Cargo.lock ./Cargo.lock
COPY ./Cargo.toml ./Cargo.toml

# this build step will cache your dependencies
RUN cargo build --release
RUN rm src/*.rs

# copy your source tree
COPY ./src ./src
# copy migrations
COPY ./migrations ./migrations

# build for release
RUN touch src/main.rs && cargo build --release

# our final base
FROM rust:bookworm

WORKDIR /bottle

RUN apt install libpq-dev

# copy the build artifact from the build stage
COPY --from=build /bottle/target/release/bottle ./bottle
# copy res & .env
COPY ./res ./res
COPY "./.env" "./.env"

# set the startup command to run your binary
CMD ["./bottle"]