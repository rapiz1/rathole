## Systemd Unit Examples

The directory lists some systemd unit files for example, which can be used to run `rathole` as a service on Linux.

[The `@` symbol in the name of unit files](https://superuser.com/questions/393423/the-symbol-and-systemctl-and-vsftpd) such as
`rathole@.service` facilitates the management of multiple instances of `rathole`.

For the naming of the example, `ratholes` stands for `rathole --server`, and `ratholec` stands for `rathole --client`, `rathole` is just `rathole`.

For security, it is suggested to store configuration files with permission `600`, that is, only the owner can read the file, preventing arbitrary users on the system from accessing the secret tokens.

### With root privilege

Assuming that `rathole` is installed in `/usr/bin/rathole`, and the configuration file is in `/etc/rathole/app1.toml`, the following steps show how to run an instance of `rathole --server` with root.

1. Create a service file.

```bash
sudo cp ratholes@.service /etc/systemd/system/
```

2. Create the configuration file `app1.toml`.

```bash
sudo mkdir -p /etc/rathole
# And create the configuration file named `app1.toml` inside /etc/rathole
```

3. Enable and start the service.

```bash
sudo systemctl daemon-reload # Make sure systemd find the new unit
sudo systemctl enable ratholes@app1 --now
```

### Without root privilege

Assuming that `rathole` is installed in `~/.local/bin/rathole`, and the configuration file is in `~/.local/etc/rathole/app1.toml`, the following steps show how to run an instance of `rathole --server` without root.

1. Edit the example service file as...

```txt
# with root
# ExecStart=/usr/bin/rathole -s /etc/rathole/%i.toml
# without root
ExecStart=%h/.local/bin/rathole -s %h/.local/etc/rathole/%i.toml
```

2. Create a service file.

```bash
mkdir -p ~/.config/systemd/user
cp ratholes@.service ~/.config/systemd/user/
```

3. Create the configuration file `app1.toml`.

```bash
mkdir -p ~/.local/etc/rathole
# And create the configuration file named `app1.toml` inside ~/.local/etc/rathole
```

4. Enable and start the service.

```bash
systemctl --user daemon-reload # Make sure systemd find the new unit
systemctl --user enable ratholes@app1 --now
```

### Run multiple services

To run multiple services at once, simply add another configuration, say `app2.toml` under `/etc/rathole` (`~/.local/etc/rathole` for non-root), then run `sudo systemctl enable ratholes@app2 --now` (`systemctl --user enable ratholes@app2 --now` for non-root) to start an instance for that configuration.

The same applies to `ratholec@.service` for `rathole --client` and `rathole@.service` for `rathole`.
