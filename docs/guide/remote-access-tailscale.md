# Remote Access With Tailscale

Wardian is local-first: agent workspaces, terminal sessions, logs, and Markdown state live on the machine running Wardian. Remote access should preserve that model. Prefer a private tailnet over opening Wardian, shell ports, or development servers to the public internet.

Use this guide when you want to check Wardian from another laptop, tablet, or phone while the host machine keeps the actual worktree, provider credentials, and terminal environment.

## Recommended Shape

Run Wardian on a trusted host, join that host and your client device to the same Tailscale tailnet, and access only the service you need:

1. Use Tailscale device-to-device connectivity for private network reachability.
2. Use MagicDNS names instead of hard-coded IP addresses when possible.
3. Use Tailscale SSH for host shell access on supported destination devices.
4. Use Tailscale Serve for a private web surface inside the tailnet.
5. Use Tailscale Funnel only for temporary public demos, never for normal operations.

Do not expose provider tokens, workspace paths, Wardian state directories, development servers, or terminal ports directly to the public internet.

## Prerequisites

- A Wardian host machine with the desktop app or development server running.
- Tailscale installed on the Wardian host and on each client device.
- A tailnet where you can manage device names, access rules, and HTTPS settings.
- Local OS accounts and filesystem permissions already set up on the Wardian host.

Install Tailscale using the official platform-specific installer, then sign in on each device. On Linux hosts, the initial CLI connection commonly needs elevated privileges unless operator permissions are configured:

```sh
sudo tailscale up
tailscale status
```

On macOS or other systems where the Tailscale CLI can manage state without elevation, omit `sudo`.

PowerShell:

```powershell
tailscale up
tailscale status
```

For unattended hosts, prefer a tagged device and a scoped auth key rather than signing in with a personal browser session. Keep key expiry enabled unless you have a documented operational reason to turn it off.

## Name The Wardian Host

Give the host a stable Tailscale machine name such as `wardian-host`. With MagicDNS enabled, clients can use the short name:

```sh
tailscale ping wardian-host
```

PowerShell:

```powershell
tailscale ping wardian-host
```

If MagicDNS is unavailable, use the host's Tailscale IP from `tailscale status`. Avoid writing that IP into shared docs; use `<wardian-host-tailnet-name>` or `<wardian-host-tailscale-ip>` placeholders.

## Private Shell Access

Use SSH over the tailnet when you need to inspect the host, run the Wardian CLI, or restart a dev process.

For Linux hosts, and for macOS hosts using the open source `tailscale` and `tailscaled` CLI variant, Tailscale SSH can manage SSH authentication and authorization:

```sh
sudo tailscale up --ssh
ssh <host-user>@wardian-host
```

PowerShell client:

```powershell
ssh <host-user>@wardian-host
```

Tailscale SSH access depends on the tailnet policy file. Keep the default self-device policy for personal use, or create a narrower SSH rule for shared Wardian hosts. Avoid rules that let broad groups SSH as arbitrary non-root users on tagged hosts.

For Windows hosts, use the normal Windows OpenSSH Server or remote desktop tooling over the Tailscale network instead of assuming Tailscale SSH can run on the destination. Keep Windows Firewall scoped to the Tailscale interface or the tailnet address range where possible.

## Private Web Access

If Wardian exposes a local web surface on the host, bind it to localhost and publish it privately with Tailscale Serve. This keeps access inside the tailnet and lets Tailscale access rules still apply.

Example for a local service on port `3000`:

```sh
tailscale serve 3000
```

PowerShell:

```powershell
tailscale serve 3000
```

The command prints a tailnet HTTPS URL such as:

```text
https://<wardian-host>.<tailnet-name>.ts.net
```

Use that URL from another device signed in to the same tailnet. If the command prompts you to enable HTTPS certificates for the tailnet, complete that consent flow in the Tailscale admin console.

Stop serving when the remote session is over:

```sh
tailscale serve reset
```

PowerShell:

```powershell
tailscale serve reset
```

## Temporary Public Demo Access

Tailscale Funnel makes a local service reachable from the public internet. Use it only for short-lived demos where every stakeholder understands that the URL is public.

Before using Funnel:

- Confirm the surface has its own authentication and does not reveal provider credentials, terminals, logs, or workspace file contents.
- Confirm you are comfortable using Funnel as a beta Tailscale feature.
- Confirm the tailnet policy allows Funnel only for the operators who need it.
- Prefer a disposable demo workspace and a mock provider.
- Set a time box and reset Funnel immediately after the demo.

Funnel requires the tailnet's HTTPS and Funnel policy prerequisites. If the CLI opens an admin consent flow, inspect the resulting tailnet policy before the demo. Do not leave a broad default such as `autogroup:member` with the `funnel` node attribute if only a smaller operator group should be allowed to publish public URLs.

Do not configure Serve and Funnel on the same tailnet URL port at the same time. Tailscale treats the most recent Serve or Funnel command for that port as authoritative, so accidentally rerunning `tailscale funnel` can make a previously private Serve endpoint public.

Example for a public demo of a local service on port `3000`:

```sh
tailscale funnel 3000
```

PowerShell:

```powershell
tailscale funnel 3000
```

Reset when finished:

```sh
tailscale funnel reset
```

PowerShell:

```powershell
tailscale funnel reset
```

Do not use Funnel for routine remote control of active Wardian agent sessions.

## CLI And Shared State

When testing the desktop app and CLI together from a remote shell, both processes must use the same `WARDIAN_HOME`.

POSIX shell:

```sh
export WARDIAN_HOME="<absolute-wardian-home-path>"
npm run dev
```

Second shell on the same host:

```sh
export WARDIAN_HOME="<absolute-wardian-home-path>"
cargo run -p wardian-cli -- agent list --scope all
```

PowerShell:

```powershell
$env:WARDIAN_HOME = "<absolute-wardian-home-path>"
npm run dev
```

Second PowerShell on the same host:

```powershell
$env:WARDIAN_HOME = "<absolute-wardian-home-path>"
cargo run -p wardian-cli -- agent list --scope all
```

Do not point a remote client at a production `WARDIAN_HOME` while running experiments. Use an isolated state directory for testing:

```sh
WARDIAN_HOME="$(mktemp -d)" npm run tauri dev
```

PowerShell:

```powershell
$env:WARDIAN_HOME = Join-Path $PWD ".tmp/wardian-remote-test"
npm run tauri dev
```

## Access Control Checklist

Before relying on remote access:

- Confirm `tailscale status` shows the expected host and client devices.
- Confirm `tailscale ping <wardian-host>` succeeds from the client.
- Confirm the Wardian service is reachable only through the tailnet path you intended.
- Confirm the host firewall does not expose the same service on a public interface.
- Confirm tailnet ACLs restrict access to the smallest practical user or group set.
- Confirm SSH access rules do not grant broad access to shared host accounts.
- Confirm no `.env`, provider token, or credential file is served through a web surface.
- Confirm Serve or Funnel has been reset when it is no longer needed.

## Troubleshooting

If a client cannot reach the host:

1. Run `tailscale status` on both devices and confirm both are signed in.
2. Run `tailscale ping <wardian-host>` to check tailnet connectivity.
3. Try the host's full MagicDNS name if the short name does not resolve.
4. Check the tailnet access rules for device-to-device, SSH, Serve, or Funnel restrictions.
5. Check the host firewall and whether the Wardian service is bound to `127.0.0.1` or another interface.
6. Reset stale Serve or Funnel state, then publish the intended service again.

If remote CLI state looks wrong, verify that the desktop app and CLI share the same `WARDIAN_HOME`. A remote shell that defaults to another home directory can read a different state database than the running app.

## References

- Tailscale install guide: <https://tailscale.com/docs/install>
- Tailscale MagicDNS: <https://tailscale.com/docs/features/magicdns/>
- Tailscale SSH: <https://tailscale.com/docs/features/tailscale-ssh>
- Tailscale Serve: <https://tailscale.com/docs/features/tailscale-serve>
- Tailscale Funnel: <https://tailscale.com/docs/features/tailscale-funnel>
