## Sync mode - not supported
To keep the library simple.

## IRQ Handler
- Simply reads USBSTS and clears write-to-clear bits
- Stores new updates in IRQ Handler-Runner Shared State

## IRQ Handler-Runner Shared State
- A bunch of atomic bools for things like pending USB int, pending USB error int, pending async advance, etc.

## Runner
- Reads updates from IRQ Handler-Runner Shared State
- Broadcasts / fans out events to other structs
- Receives requests from devices to remove a QH
- Handles ringing doorbell, receiving async advance status, and sending responses to devices indicating the QH is removed
- Similar thing for periodic list
- Receives requests from ports to reset a port and set the device address to not 0, does the reset, and then responds

## Root Port
- Allows OS can configure 
  - wake on over-current enable 
  - wake on disconnect enable
  - wake on connect eanble
  - port indicator control
  - power power
  - reset port (request sent to runner). if a device is connected then the device will be returned.
  - configure suspend?
  - force port resume?
  - disable port?

## Device
- Either has an address or is disconnected
