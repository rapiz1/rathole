# Benchmark

> Date: 2021/12/28
>
> Version: commit 1180c7e538564efd69742f22e77453a1b74a5ed2
> 
> Arch Linux with 5.15.11-arch2-1 kernel
>
> Intel Xeon CPU E5-2620 @ 2.00GHz *2
>
> 16GB RAM

## Bandwidth

![tcp_bitrate](./img/tcp_bitrate.svg)
![udp_bitrate](./img/udp_bitrate.svg)

rathole with the following configuration:
```toml
[client]
remote_addr = "localhost:2333"
default_token = "123"

[client.services.bench-tcp]
local_addr = "127.0.0.1:80"
[client.services.bench-udp]
type = "udp"
local_addr = "127.0.0.1:80"

[server]
bind_addr = "0.0.0.0:2333"
default_token = "123"

[server.services.bench-tcp]
bind_addr = "0.0.0.0:5202"
[server.services.bench-udp]
type = "udp"
bind_addr = "0.0.0.0:5202"
```

frp 0.38.0 with the following configuration:
```ini
[common]
bind_port = 7000
authentication_method = token
token = 1233
```
```ini
# frpc.ini
[common]
server_addr = 127.0.0.1
server_port = 7000
authentication_method = token
token = 1233

[bench-tcp]
type = tcp
local_ip = 127.0.0.1
local_port = 80
remote_port = 5203
[bench-udp]
type = udp
local_ip = 127.0.0.1
local_port = 80
remote_port = 5203
```

```
$ iperf3 -v
iperf 3.10.1 (cJSON 1.7.13)
Linux sig 5.15.7-arch1-1 #1 SMP PREEMPT Wed, 08 Dec 2021 14:33:16 +0000 x86_64
Optional features available: CPU affinity setting, IPv6 flow label, TCP congestion algorithm setting, sendfile / zerocopy, socket pacing, authentication, bind to device, support IPv4 don't fragment
$ sudo iperf3 -s -p 80
```

For rathole benchmark:
```
$ iperf3 -c 127.0.0.1 -p 5202
```

For frp benchmark:
```
$ iperf3 -c 127.0.0.1 -p 5203
```

## HTTP

nginx/1.20.2 listens on port 80, with the default test page.

frp and rathole configuration is same with the previous section.

[vegeta](https://github.com/tsenart/vegeta) is used to generate HTTP load.

### HTTP Throughput

The following commands are used to benchmark rathole and frp. Note that if you want to do a benchmark yourself, `-max-workers` should be adjusted to get the accurate results for your machine.

```
echo 'GET http://127.0.0.1:5203' | vegeta attack -rate 0 -duration 30s -max-workers 48
echo 'GET http://127.0.0.1:5202' | vegeta attack -rate 0 -duration 30s -max-workers 48
```

![http_throughput](./img/http_throughput.svg)

### HTTP Latency

`rathole` has very similar latency to `frp`, but can handle more connections

Here's a table, latency is in ms

|QPS|latency(rathole)|latency(frp)|
|--|--|---|
|1|2.113|2.55|
|1000|1.723|1.742|
|2000|1.845|1.749|
|3000|2.064|2.011|
|4000|2.569|7907|

As you can see, for QPS from 1 to 3000, rathole and frp have nearly identical latency.
But with QPS of 4000, frp starts reporting lots of errors and the latency grows to even seconds. This kind of reflects the throughput in the previous section.

Thus, in terms of latency, rathole and frp are nearly the same. But rathole can handle more connections.

[Script to benchmark latency](../benches/scripts/http/latency.sh)

## Memory Usage

![mem](./img/mem-graph.png)

The graph shows the memory usage of frp and rathole when `vegeta attack -duration 30s -rate 1000` is executed.
rathole uses much less memory than frp.

[Script to benchmark memory](../benches/scripts/mem/mem.sh)
