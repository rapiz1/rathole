## Systemd Unit Examples

The directory lists some systemd unit files for example, which can be used to run `rathole` as a service on Linux.

[The `@` symbol in name of unit files](https://superuser.com/questions/393423/the-symbol-and-systemctl-and-vsftpd) such as
`rathole@.service` facilitates the management of multiple instances of `rathole`.

For the naming of the example, `ratholes` stands for `rathole --server`, and `ratholec` stands for `rathole --client`, `rathole` is just `rathole`.

Assuming that `rathole` is installed in `/usr/bin/rathole`, and the configuration file is in `/etc/rathole/app1.toml`, the following steps shows how to run an instance of `rathole --server`.

1. Create a service file.

```bash
sudo cp ratholes@.service /etc/systemd/system/
```

2. Create the configuration file `app1.toml`.

```bash
sudo mkdir -p /etc/rathole
# And create the configuration file named `app1.toml` inside /etc/rathole
```

3. Enable and start the service

```bash
sudo systemctl daemon-reload # Make sure systemd find the new unit
sudo systemctl enable ratholes@app1 --now
```

And if there's another configuration named `app2.toml` in `/etc/rathole`, then
`sudo systemctl enable ratholes@app2 --now` can start an instance for that configuration.

The same applies to `rathole --client` and `rathole`.
