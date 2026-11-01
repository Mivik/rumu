---
keywords: [deploy, service, systemd, logs]
description: A sample service card with config path, restart and logs.
---

The example service runs on host `service`.

- Config: `/etc/example/config.conf`; restart with `systemctl restart example`.
- Check logs with `journalctl -u example -f`.
- Validate the config before restarting.
