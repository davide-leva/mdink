.PHONY: build static test package package-static clean

build:
	cargo build --release --locked
	mkdir -p dist
	cp target/release/mdink dist/mdink

test:
	cargo build --release --locked
	MDINK_BIN=target/release/mdink cargo test --locked

package: build
	tar -czf dist/mdink-$$(rustc -vV | sed -n 's/^host: //p').tar.gz -C dist mdink -C .. README.md native/fonts/LICENSE

static:
	CC_x86_64_unknown_linux_musl=gcc cargo build --release --locked --target x86_64-unknown-linux-musl
	mkdir -p dist
	cp target/x86_64-unknown-linux-musl/release/mdink dist/mdink

package-static: static
	tar -czf dist/mdink-x86_64-unknown-linux-musl.tar.gz -C dist mdink -C .. README.md native/fonts/LICENSE

clean:
	cargo clean
	rm -rf dist
