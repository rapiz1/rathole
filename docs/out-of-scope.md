# Out of Scope

`rathole` focuses on the forwarding for the NAT traversal, rather than being a all-in-one development tool or a load balancer or a gateway. It's designed to *be used with them*, not *replace them*.

But that doesn't mean it's not useful for other purposes. In the future, more configuration APIs will be added and `rathole` can be used with an external dashboard.

> Make each program do one thing well.

- *Domain based forwarding for HTTP*

  Introducing these kind of features into `rathole` itself ultimately reinvent a nginx. Use nginx to do this and set `rathole` as the upstream. This method achieves better performance as well as flexibility.

- *HTTP Request Logging*

  `rathole` doesn't interference with the application layer traffic. A right place for this kind of stuff is the web server, and a network capture tool.

- *`frp`'s STCP*

  You may want to consider secure tunnels like wireguard or zerotier.

- *P2P hole punching*

  P2P hole punching requires setup on the visitors' side. If that kind of setup is possible, then there are a lot more tools available. `rathole` primarily focuses on NAT traversal by forwarding, which doesn't require any setup for visitors.
