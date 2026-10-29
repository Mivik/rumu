---
title: ssh-agent setup
description: Fixing 'Permission denied (publickey)' — agent check, keys, agent service.
---
When git over ssh fails with "Permission denied (publickey)":

1. Check the agent: `ssh-add -l`. If it lists no keys, add one:
   `ssh-add ~/.ssh/id_ed25519`.
2. If the agent isn't running (empty SSH_AUTH_SOCK), start it inside the
   session's environment or use the systemd user service
   (`~/.config/systemd/user/ssh-agent.service`).
3. Test before retrying the big operation: `ssh -T git@github.com`.
