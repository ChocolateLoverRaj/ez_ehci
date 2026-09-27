This is a driver for using a eHCI (USB 2.0 Host Controller) in your OS. Currently it's made for the kernel itself to use with an async executor. Upon request it can be modified to work better with microkernels or kernels that want to provide a USB API.

## QH Management
- Anchor QH
  - Always the H=1 QH, always in the circular list
  - Next addr gets updated as QHs are added and removed

### Adding a QH
- Read anchor QH next
- Prepare QH to add with next that's the same as anchor QH next
- Set anchor QH next to QH to add

Strategy for concurrnet adds:
- Read anchor QH next
- Write QH next to QH to add
- Compare and swap expected anchor QH next with ptr to new QH
- If it failed, try again

### Removing a QH
- Need to update previous QH somehow
- Can store pointer to prev in the QH, making the list doubly linked

### Adding a QH while doubly linked
- Anchor QH doubly links to itself
- All new QHs have prev is anchor QH
- Read anchor QH next