# io-to-net

io-to-net is a universal hardware IO bridge that exposes local device interfaces over TCP/IP.

The application allows any locally connected hardware interface to be accessed remotely as if it were connected locally.

Typical use cases include:

- exposing serial devices over the network
- bridging sensor hardware to remote processing nodes
- accessing ECU telemetry remotely
- making I2C / GPIO / Bluetooth devices available across a network
- enabling cloud ingestion pipelines

io-to-net is designed to integrate with the existing telemetry ecosystem:

speeduino-to-mqtt
gps-to-mqtt
sensors-to-mqtt

The bridge acts as the transport layer underneath these applications.

Example architecture:

device → io-to-net → TCP → sensors-to-mqtt → MQTT → telemetry apps

The bridge must provide stable, low-latency streaming of binary or text protocols.

Key design principles:

- reliability
- minimal latency
- protocol transparency
- strong observability
- cross-platform operation