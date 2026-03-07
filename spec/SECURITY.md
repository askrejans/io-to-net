# Security Policy

This document describes the security model and recommended deployment practices for io-to-net.

io-to-net exposes hardware interfaces over network connections.  
Improper configuration may expose sensitive devices or control interfaces.

Users must carefully review the following guidelines.

---

# Security Model

io-to-net acts as a transparent bridge between hardware interfaces and network clients.

Example:

hardware device → io-to-net → TCP/IP → remote client

The bridge itself does not interpret protocol data.

Security must therefore be enforced at the transport and network layers.

---

# Recommended Deployment

The safest deployment model is:

```
hardware device → io-to-net → localhost → application
```

Applications running on the same machine connect to io-to-net through localhost.

---

# Network Exposure Risks

Exposing hardware interfaces over a network may allow:

device control  
data extraction  
firmware modification  
protocol injection  

Examples include:

ECU diagnostic ports  
industrial equipment  
embedded debug consoles  

Users should never expose io-to-net directly to the public internet.

---

# Transport Security

Secure deployments should use TLS.

Configuration example:

```
[tls]
enabled = true
cert = "/etc/io-to-net/cert.pem"
key = "/etc/io-to-net/key.pem"
```

TLS encrypts all communication between clients and the bridge.

---

# Authentication

Optional token-based authentication may be enabled.

Example:

```
[auth]
token = "secure-token"
```

Clients must provide:

```
Authorization: Bearer secure-token
```

---

# Network Isolation

io-to-net should run within trusted networks only.

Recommended environments:

local machines  
private networks  
VPN environments  
container networks  

Never expose the service directly to public internet access.

---

# IP Access Controls

Deployments should restrict access using firewalls.

Example firewall rules:

allow local subnet only  
block external access  

Example:

```
iptables allow 192.168.1.0/24
```

---

# Read-Only Mode

Some hardware should only be monitored.

Bridges may be configured in read-only mode to prevent command injection.

Example:

```
mode = "read_only"
```

In this mode clients cannot send data to the device.

---

# Rate Limiting

To prevent denial-of-service attacks, deployments may enable connection limits.

Example:

```
max_clients = 10
```

Connections beyond this limit will be rejected.

---

# Logging and Monitoring

io-to-net provides runtime metrics and logs.

Monitoring should include:

connection attempts  
client IP addresses  
bridge errors  
unexpected traffic spikes  

Prometheus metrics can help detect abnormal behavior.

---

# Reporting Security Issues

Security vulnerabilities should be reported privately.

Please open a security report via the repository's security advisory process.

Do not publicly disclose vulnerabilities before maintainers have had time to address them.

---

# Responsible Usage

This software should be used only on systems where the operator has authorization to access the hardware interfaces.

Unauthorized access to devices may violate laws or regulations.

---

# Summary

Safe deployments should include:

TLS encryption  
authentication tokens  
firewall restrictions  
private network placement  
monitoring and logging  

These measures help prevent unauthorized access to connected hardware.