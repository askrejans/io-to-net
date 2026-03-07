# HTTP Admin API

io-to-net must expose an optional HTTP administrative API.

The API allows external tools, automation systems, and dashboards to inspect and control the running service.

The HTTP API is read-mostly and intended for observability and management.

Default endpoint:

http://localhost:9106

Example configuration:

[http_api]
enabled = true
listen = "0.0.0.0:9106"

------------------------------------------------

IMPLEMENTATION

Recommended Rust frameworks:

axum  
hyper  

The HTTP server must run in its own async task and must not block bridge processing.

Responses must support:

JSON  
plain text where appropriate

------------------------------------------------

API ENDPOINTS

GET /health

Returns service health status.

Example response:

{
  "status": "ok",
  "uptime_seconds": 1241
}

------------------------------------------------

GET /bridges

Returns a list of configured bridges.

Example response:

{
  "bridges": [
    {
      "name": "ecu",
      "source_type": "serial",
      "source": "/dev/ttyUSB0",
      "transport": "tcp",
      "listen": "[::]:9000",
      "clients": 2,
      "bytes_rx": 124234,
      "bytes_tx": 212341
    }
  ]
}

------------------------------------------------

GET /bridges/{name}

Returns detailed information about a specific bridge.

Example:

GET /bridges/ecu

Response:

{
  "name": "ecu",
  "source_type": "serial",
  "source": "/dev/ttyUSB0",
  "baud": 115200,
  "transport": "tcp",
  "listen": "[::]:9000",
  "clients": 2,
  "bytes_rx": 124234,
  "bytes_tx": 212341,
  "errors": 0
}

------------------------------------------------

GET /connections

Returns all active client connections.

Example response:

{
  "connections": [
    {
      "client_ip": "192.168.1.50",
      "bridge": "ecu",
      "connected_seconds": 120,
      "bytes_rx": 22000,
      "bytes_tx": 18000
    }
  ]
}

------------------------------------------------

GET /metrics

Returns the same Prometheus metrics exposed by the metrics endpoint.

This endpoint may internally reuse the Prometheus metrics exporter.

------------------------------------------------

GET /logs

Returns recent log entries.

Optional query parameters:

limit

Example:

GET /logs?limit=50

Response:

{
  "logs": [
    "2026-03-07T10:21:11Z client connected 192.168.1.50",
    "2026-03-07T10:21:14Z serial read 128 bytes"
  ]
}

------------------------------------------------

POST /bridges/{name}/restart

Restarts a specific bridge.

Example:

POST /bridges/ecu/restart

Response:

{
  "status": "restarted"
}

------------------------------------------------

POST /reload-config

Reloads configuration file without restarting the entire service.

Example:

POST /reload-config

Response:

{
  "status": "reloaded"
}

------------------------------------------------

SECURITY

The admin API may optionally support authentication.

Possible methods:

token header  
basic auth  

Example configuration:

[http_api.auth]
token = "secret-token"

Requests must include:

Authorization: Bearer secret-token

------------------------------------------------

RATE LIMITING

The API should optionally support rate limiting to prevent abuse.

Example configuration:

[http_api]
rate_limit_per_second = 20

------------------------------------------------

USE CASES

Monitoring bridges remotely

Integration with Prometheus and Grafana

Remote management scripts

Automation and orchestration

Future web UI control panel

------------------------------------------------

FUTURE EXTENSIONS

WebSocket event stream

Example endpoint:

GET /events

Used to stream:

client connect events  
client disconnect events  
bridge restart events  

------------------------------------------------

END OF DOCUMENT