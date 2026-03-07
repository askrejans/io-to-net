# TUI Interface

The TUI is built using:

ratatui
crossterm

Layout:

header
tabs
main
logs

Header shows:

version
active bridges
total clients
total bandwidth

Tabs:

Ports
Connections
Stats
Logs

Ports tab:

list of configured bridges
status indicator
connected clients
traffic rate

Connections tab:

client address
connected bridge
bytes in
bytes out
connection duration

Stats tab:

global bandwidth
error counters
active bridges

Logs tab:

recent tracing logs