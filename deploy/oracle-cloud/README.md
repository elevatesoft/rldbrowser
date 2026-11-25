# Deploying `rld-proctor` on Oracle Cloud Always Free (ARM64)

This guide deploys `rld-proctor` on an **Oracle Cloud Infrastructure (OCI) Always Free Ampere A1 (ARM64) VM** running **Ubuntu 24.04 LTS**, with Caddy handling automatic Let's Encrypt TLS, HTTP/2, and WebSockets in front of a localhost-only `rld-proctor` systemd service.

---

## 1. Create the OCI VM & Configure VCN Security List

1. In the OCI Console, create a Compute Instance:
   - **Image:** Canonical Ubuntu 24.04 (aarch64)
   - **Shape:** `VM.Standard.A1.Flex` (Always Free eligible: e.g., 1–2 OCPU, 6–12 GB RAM)
   - **SSH Keys:** Upload or generate an SSH key pair (keep SSH key-only; do not enable password auth).
2. Open **Networking → Virtual Cloud Networks → `<your-vcn>` → Security Lists → Default Security List → Ingress Rules** and ensure only the required TCP ports are open:
   - **TCP 22** (SSH — restrict Source CIDR to your IP if possible)
   - **TCP 80** (`0.0.0.0/0` — required for Let's Encrypt HTTP-01 challenge & HTTPS redirect)
   - **TCP 443** (`0.0.0.0/0` — HTTPS)
   - Do **not** open port `8443` publicly.

---

## 2. Configure UFW Firewall on the VM

SSH into the VM and enable `ufw` for SSH, HTTP, and HTTPS only:

```bash
sudo apt update && sudo apt upgrade -y
sudo ufw default deny incoming
sudo ufw default allow outgoing
sudo ufw allow 22/tcp
sudo ufw allow 80/tcp
sudo ufw allow 443/tcp
sudo ufw enable
sudo ufw status verbose
```

> **Note for OCI Ubuntu images:** Oracle's default Ubuntu images include an `iptables` `REJECT` rule in `/etc/iptables/rules.v4` that can block ports 80/443 even after enabling UFW. If external requests to 80/443 time out or are refused, remove the `REJECT` rule or disable `netfilter-persistent` so UFW manages the firewall cleanly.

---

## 3. Point DNS to the OCI Public IP

Create a DNS **A** record for your domain pointing to the VM's public IPv4 address:

```text
Type: A
Name: proctor.yourdomain.com
Value: <OCI_PUBLIC_IPV4>
TTL: Auto / 300
```

Verify propagation before starting Caddy:

```bash
dig +short A proctor.yourdomain.com
```

---

## 4. Install Rust, Build, and Install `rld-proctor`

Install build prerequisites and Rust (via `rustup`):

```bash
sudo apt install -y build-essential git curl pkg-config
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source "$HOME/.cargo/env"
```

Clone the repository, build the release binary for `rustbrowser-proctor`, and install it to `/usr/local/bin/rld-proctor`:

```bash
git clone https://github.com/elevatesoft/rldbrowser.git
cd rldbrowser
cargo build --release -p rustbrowser-proctor
sudo install -m 0755 target/release/rustbrowser-proctor /usr/local/bin/rld-proctor
```

---

## 5. Create the Service User & Enable `rld-proctor.service`

Create a dedicated non-root system user and data directory:

```bash
sudo useradd --system --no-create-home --shell /usr/sbin/nologin rld-proctor
sudo mkdir -p /var/lib/rld-proctor
sudo chown -R rld-proctor:rld-proctor /var/lib/rld-proctor
sudo chmod 0750 /var/lib/rld-proctor
```

Install the hardened systemd unit and start the service:

```bash
sudo cp deploy/oracle-cloud/rld-proctor.service /etc/systemd/system/rld-proctor.service
sudo systemctl daemon-reload
sudo systemctl enable --now rld-proctor.service
sudo systemctl status rld-proctor.service
```

Confirm it is listening only on loopback (`127.0.0.1:8443`):

```bash
ss -tlnp | grep 8443
```

---

## 6. Install Caddy, Configure TLS, and Verify

Install the official Caddy package for Ubuntu:

```bash
sudo apt install -y debian-keyring debian-archive-keyring apt-transport-https curl
curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/gpg.key' | sudo gpg --dearmor -o /usr/share/keyrings/caddy-stable-archive-keyring.gpg
curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/debian.deb.txt' | sudo tee /etc/apt/sources.list.d/caddy-stable.list
sudo apt update
sudo apt install -y caddy
```

Copy the `Caddyfile`, replace `proctor.example.com` with your domain, validate, and reload Caddy:

```bash
sudo cp deploy/oracle-cloud/Caddyfile /etc/caddy/Caddyfile
sudo sed -i 's/proctor\.example\.com/proctor.yourdomain.com/g' /etc/caddy/Caddyfile
sudo caddy validate --config /etc/caddy/Caddyfile
sudo systemctl reload caddy
```

Verify HTTPS, security headers, and service health:

```bash
curl -fsSI https://proctor.yourdomain.com/
```

Check logs at any time with:

```bash
sudo journalctl -u rld-proctor -u caddy -f
```
