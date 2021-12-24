# rathole
![rathole-logo](./docs/img/rathole-logo.png)

A fast and stable reverse proxy for NAT traversal, written in Rust

rathole, like [frp](https://github.com/fatedier/frp), can help to expose the service on the device behind the NAT to the Internet, via a server with a public IP.

## Quickstart

To use rathole, you need a server with a public IP, and a device behind the NAT, where some services that need to be exposed to the Internet. 

Assuming you have a NAS at home behind the NAT, and want to expose its ssh service to the Internet:

1. On the server which has a public IP

Create `server.toml` with the following content and accommodate it to your needs.
```toml
# server.toml
[server]
bind_addr = "0.0.0.0:2333" # `2333` specifys the port that rathole listens for clients

[server.services.my_nas_ssh]
token = "use_a_secret_that_only_you_know" # Token that is used to authenticate the client for the service. Change to a arbitrary value.
bind_addr = "0.0.0.0:5202" # `5202` specifys the port that exposes `my_nas_ssh` to the Internet
```

Then run:
```bash
./rathole server.toml
```

2. On the host which is behind the NAT (your NAS)

Create `client.toml` with the following content and accommodate it to your needs.
```toml
[client]
remote_addr = "myserver.com:2333" # The address of the server. The port must be the same with the port in `server.bind_addr`

[client.services.my_nas_ssh]
token = "use_a_secret_that_only_you_know" # Must be the same with the server to pass the validataion
local_addr = "127.0.0.1:22" # The address of the service that needs to be forwarded
```

Then run:
```bash
./rathole client.toml
```

3. Now the client will try to connect to the server `myserver.com` on port `2333`, and any traffic to `myserver.com:5202` will be forwarded to the client's port `22`.

So you can `ssh myserver.com:5202` to ssh to your NAS.

## Configuration
`rathole` can automatically determine to run in the server mode or the client mode, according to the content of the configuration file, if only one of `[server]` and `[client]` block is present, like the example in [Quickstart](#Quickstart).

But the `[client]` and `[server]` block can also be put in one file. Then on the server side, run `rathole --server config.toml` and on the client side, run `rathole --client config.toml` to explictly tell `rathole` the running mode.

Some configuration examples are provided under [examples](./examples).

The Noise Protocol can be easily used to secure the traffic, see [Security](./docs/security.md).

Here is the full configuration specification:
```toml
[client]
remote_addr = "example.com:2333" # Necessary. The address of the server
default_token = "default_token_if_not_specify" # Optional. The default token of services, if they don't define their own ones

[client.transport]
type = "tcp" # Optional. Possible values: ["tcp", "tls"]. Default: "tcp"

[client.transport.tls] # Necessary if `type` is "tls"
trusted_root = "ca.pem" # Necessary. The certificate of CA that signed the server's certificate
hostname = "example.com" # Optional. The hostname that the client uses to validate the certificate. If not set, fallback to `client.remote_addr`

[client.transport.noise] # Noise protocol. See `docs/security.md` for further explanation
pattern = "Noise_NK_25519_ChaChaPoly_BLAKE2s" # Optional. Default value as shown
local_private_key = "key_encoded_in_base64" # Optional
remote_public_key = "key_encoded_in_base64" # Optional

[client.services.service1] # A service that needs forwarding. The name `service1` can change arbitrarily, as long as identical to the name in the server's configuration
type = "tcp" # Optional. The protocol that needs forwarding. Possible values: ["tcp", "udp"]. Default: "tcp"
token = "whatever" # Necessary if `client.default_token` not set
local_addr = "127.0.0.1:1081" # Necessary. The address of the service that needs to be forwarded

[client.services.service2] # Multiple services can be defined
local_addr = "127.0.0.1:1082"

[server]
bind_addr = "0.0.0.0:2333" # Necessary. The address that the server listens for clients. Generally only the port needs to be change. 
default_token = "default_token_if_not_specify" # Optional

[server.transport]
type = "tcp" # Same as `[client.transport]`

[server.transport.tls] # Necessary if `type` is "tls"
pkcs12 = "identify.pfx" # Necessary. pkcs12 file of server's certificate and private key
pkcs12_password = "password" # Necessary. Password of the pkcs12 file

[server.transport.noise] # Same as `[client.transport.noise]`
pattern = "Noise_NK_25519_ChaChaPoly_BLAKE2s"
local_private_key = "key_encoded_in_base64" 
remote_public_key = "key_encoded_in_base64" 

[server.services.service1] # The service name must be identical to the client side
type = "tcp" # Optional. Same as the client `[client.services.X.type]
token = "whatever" # Necesary if `server.default_token` not set
bind_addr = "0.0.0.0:8081" # Necessary. The address of the service is exposed at. Generally only the port needs to be change. 

[server.services.service2] 
bind_addr = "0.0.0.1:8082"
```

### Logging
`rathole`, like many other Rust programs, use environment variables to control the logging level. `info`, `warn`, `error`, `debug`, `trace` are avialable.

```
RUST_LOG=error ./rathole config.toml
```
will run `rathole` with only error level logging.

If `RUST_LOG` is not present, the default logging level is `info`.

## Benchmark

rathole has similiar latency to [frp](https://github.com/fatedier/frp), but can handle more connections. Also it can provide much better bandwidth than frp.

See also [Benchmark](./docs/benchmark.md).

![tcp_bitrate](./docs/img/tcp_bitrate.svg)

![tcp_latency](./docs/img/tcp_latency.svg)

## Development Status

`rathole` is in active development. A load of features is on the way:
- [x] TLS support
- [x] UDP support
- [ ] Hot reloading
- [ ] HTTP APIs for configuration
