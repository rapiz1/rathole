## Systemd Configuration

We provide various systemd examples to make the management of rathole easy. You can find out various services file in
the current directory.

Here we will try to install server version. Same will apply for client etc.

Before procedding we need to have configuration ready. For that please refer to readme file. Also, @ in filename such as
rathole@.service carries [special meaning](https://superuser.com/questions/393423/the-symbol-and-systemctl-and-vsftpd) to enable multiple instances of ratholec. If you are only hosting  one instance then
feel free to use systemd config file that doesn't use @. Also, whenever we mention systemd config it means file that has *.service extension.

Here is simple instruction to install rathole server.

1. Create a service file:

```bash
wget https://github.com/rapiz1/rathole/blob/main/examples/systemd/ratholes@.service # download the file
sudo cp rathole@.service /lib/systemd/system/
```

2. Create the rathole configuration file we shall call it app1.toml.

```
sudo mkdir -p /etc/rathole
# Now create rathole config file called app1 inside /etc/rathole
```

If you don't want to use /etc/rathole you can tweak the systemd config file

```
ExecStart=/usr/bin/rathole -s /etc/rathole/%i.toml
```

_Note_: Don't replace `%i` becase it will be replaced by app1, app2 when we do `systemctl start rathole@app1` in coming
step.

3. Enable to service so it works automatically when computer is rebooted.

```bash
sudo systemctl enable ratholes@app1
```

4. Start the service

```bash
sudo systemctl enable ratholes@app1
```

You can use app1, app2 or whatever you like but make sure config file exists.
