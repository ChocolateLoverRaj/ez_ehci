This is a driver for using a eHCI (USB 2.0 Host Controller) in your OS. 

# Supported features
- eHCI with integrated rate matching hub (planned)
- Computers with >1 eHCI (just create multiple instances of this driver, there are no global variables, so it will just work)
- Detecting new devices being attached 
- Automatically assigning addresses to devices
- Detecting devices getting detached (planned)
- Getting device information and configurations
- Setting configurations
- Doing interrupt transfers with a ring buffer of QTDs (for keyboard, mouse, gamepad, and other HID device input)
- Doing control transfers on arbitrary endpoints (planned)
- Doing bulk transfers on arbitrary endpoints (planned)

# Unsupported features
- eHCI with companion controller (not not planned, but not planned either)

# Tested Devices
- QEMU q35 (eHCI, high speed devices only, no support for USB 2.0 hubs)
- Lenovo Z560 Laptop (eHCI with built-in rate matching hubs for each physical port)

# What kind of OS this is for
Right now this is made for a unikernel, which runs an async executor. It could be made to work with a microkernel, but that would require a split between the kernel side of the driver and unpriviledged side of the driver, since no computer with an eHCI has an IOMMU. It could also be made to run a monolithic kernel but without any async code.

# Lifetimes, `&T`, and `&mut T`
It's expected that once you map the eHCI, it's `'static`. Halting the eHCI and cleaning up resources is currently not supported, although this is possible to implement.

The library splits the overall statge of the eHCI into multiple components. Each component has methods which take `&mut self`, so you can do multiple things in parallel such as:
- Waiting for a new device to be attached on a root port
- For each connected device
  - Send a transfer on the control endpoint (endpoint 0)
  - Send control transfers on an endpoint
  - Send a bulk transfer on an endpoint
  - Continuously receive packets on an interrupt endpoint as a ring buffer

These structs can wait to acquire locks to do certain operations and can allocate.

The library also gives you a single IRQ handler struct, which you call with `&mut self` in your IRQ handler to process interrupts. Calling the IRQ handler function is guaranteed to not allocate (or deallocate), not acquire locks, and be O(1).

This library is designed so that you run normal functions (which are `async`) with interrupts enabled, which is why the IRQ handler is made not not acquire locks, since if it did that would cause deadlocks.

# Implementation Notes
## 32-bit vs 64-bit capability
Depending on the computer, the eHCI may or may not support 64-bit addressing.

### If it does not support 64-bit addressing
- Data structures such as QHs and QTDs must be within the lower 4 GiB of physical memory. The fields they have are the fields in the specification.
- Buffers must be within the lower 4 GiB of physical memory.
- All addresses are 32-bits.

### If it supports 64-bit addressing
- Data structures such as QHs and QTDs must be within a 4 GiB (aligned to 4 GiB) window of physical memory. This 4 GiB window doesn't have to be the lowest 4 GiB, it can be anywhere. Write to the `CTRLDSSEGMENT` to set the address of the 4 GiB window. The value will be the upper 32-bits of a 64-bit address. Pointers to data structures will be the lower 32-bits, which will be combined with the upper 32-bits to form 64-bit addresses that the eHCI will use. **Data structures have additional fields below the normal fields, which are 32-bit fields that are the upper bits of pointers to buffers. Make sure you use these fields in your structs, otherwise it will lead to host system errors.**
- Buffers can be anywhere in the 64-bit physical memory address space.

## Steps to do fun things with an eHCI
### Finding eHCI functions
Enumerate the PCI devices, looking for USB controllers. You may find an eHCI function in its own PCI device. This means that the computer has a integrated USB 2.0 hub so that you can communicate with USB 1.x devices with just this eHCI driver. 

Another possibilty is finding a PCI device which first has a uHCI or oHCI function and then an eHCI function after it. This means that you will need a oHCI or uHCI driver along with an eHCI driver to support both USB 1.x and USB 2.0 devices.

### Taking control of the eHCI from the BIOS
Computers with an eHCI will have a legacy BIOS, not UEFI. UEFI has exit boot services, but BIOS doesn't, so there isn't a built-in way for the BIOS to release control of the eHCI. There is a high chance that your BIOS supports booting off of a USB drive and supports the OS reading from the USB drive through BIOS interrupts. If you want to take control of the eHCI and use it with your OS drivers, you **must** go through the legacy handoff procedure.

USBLEGSUP is an "extended capability". The wording makes it sound like an optional feature. Not all eHCIs are required to support this, but operating systems are required to detect this capability and go through the legacy hand-off procedure, otherwise there will be unexpected behavior and contention between your OS and the BIOS. You may not realize this, but the BIOS can actually run in the background, without your OS even noticing. It does not through SMIs (System Management Interrupts). Until you take control of the eHCI, the BIOS might be continuously handling eHCI interrupts in the background.

First, read the HCCPARAMS register (an Operational Register, located in the memory referenced by BAR0). It contains an EHCI Extended Capabilities Pointer (EECP). This pointer is for a location in the **PCI Configuration Space**, below the standard PCI information such as the BARs and Command reg. You basically write a 1 as a u8 in a certain place in memory (if using PCIe access) or port I/O, which will send an SMI interrupt and transfer control to the BIOS. The BIOS will eventually update a bit, indicating that it has released ownership of the eHCI and now you as the OS own it. There is no interrupt that occurs when the BIOS updates this bit, so you will have to poll until it's released by the BIOS.

### Resetting the controller
Reset it.

### Setting up registers and data structures
- Enable interrupts
- Initialize and enable the periodic list
- Initialize and enable the async list 
- Configure the Interrupt Threshold Control in USBCMD. For the best gaming experience and most responsive OS, chose every 1 micro-frame. For computers with slow CPUs this might cause issues if you are getting constant interrupts with no time to do anything besides handling interrupts, so for those computers you can choose to have less frequent eHCI interrupts.

### Detecting devices
- Read port status
- Wait for port status change interrupts to get notified when new devices are plugged in to the root ports

### Initializing a device
- Reset the device
- Now the device has address 0. Don't reset another device while this device still has address 0 since that would create a conflict.
- Assign an address for that device. Valid addresses are from 1..=127.
- Send a command to the device so it switches to that address using the async list. To do this, you will need 1 QH and 2 QTDs. For the QTDs, you will need 1 SETUP QTD and 1 IN QTD (which is to receive the status, no data transferred in a buffer).
- Wait x ms for the device to switch to that address.
- You can now reset another device in parallel as you further communicate with this device.

### Doing cool stuff with a device
Most of these instructions are generic to the USB protocol and not specific to an eHCI.

Read the device descriptor, which will give you the number of configurations. A configuration is an operating mode of a USB device. It can support multiple configurations, and you can choose which configuration to set it to. In practice USB devices will only have 1 configuration. You do this with 3 QTDs. SETUP, IN (with a buffer of 18 B), OUT (status, no buffer). The device descriptor may be shorter than 18 bytes, which is okay. Make sure to set the Alternate QTD Pointer to also point to the OUT packet in addition to the Next QTD Pointer. If the device descriptor is less than 18 B, then the Alternate QTD Pointer will be used by the eHCI to advance to the next QTD. Either way, you want the next and final QTD to be the OUT one.

Read every configuration descriptor. Reading it is similar to reading a device descriptor, except that configuration descriptors are larger than device descriptors. The maximum length as per the USB spec is 65,535 B (64 KiB - 1 B). The slight challenge is that you don't know the size of the device descriptor until you read the first few fields of it, which contains its total length. One method of doing this is first doing a transaction to read the first few fields, and then doing a second transaction to read the full device descriptor. There are 2 places where you indicate how many bytes you want to transfer. One place is the SETUP packet, which indicates to the *USB device* how many bytes it should send you. Another place is the bytes to transfer field of the QTD. This indicates to the eHCI how many bytes to transfer for that QTD. It's okay to ask for more bytes than the device descriptor has, the device will just send the exact number of bytes the descriptor has. In theory you could allocate a 64 KiB buffer to be prepared for the maximum size, so that you can always read the entire device descriptor in 1 transaction. You can do this by using 4 QTDs, since each QTD can only reference 20 KiB of a buffer. However, this will not work on QEMU since it will error if you try to do a control transfer of >4096 B. So a more practical approach is to limit your OS to 4096 B device descriptors. You will only need a single IN QTD for this, and it will work in QEMU and in practice support all real USB devices. 

A configuration descriptor actually contains a configuration descriptor, interface descriptors, and interface descriptors contain endpoint descriptors. Interface descriptors can also contain other descriptors, such as HID descriptors. It is a tree structure but just a list of consecutive descriptors in memory. 

Choose which configuration to put the device in. Most devices will only have 1 configuration, usually with the number `1`. If there are multiple configurations, you will need to choose which one you want somehow. Even if the device only has 1 configuration, you still need to explicitly set the device to use that configuration. Issue SETUP + IN (status) QTDs to set the configuration.

Now you can use the interfaces and the endpoints. A configuration can have multiple interfaces, and this is actually the case for many common real USB devices, such as keyboards. Each interface has a group of endpoints. Each endpoint in an interface doesn't have much meaning on its own but together all of the endpoints of an interface do a certain thing. For example, there are keybaord + mouse combo devices that have 1 interface for the keybaord and 1 interface for the mouse. In your OS, you can expose these interface to USB device-specific drivers, allowing 1 USB device with multipel interfaces to be split up between different pieces of code.

A really common USB interface is a keyboard, or a mouse. These devices, along with gamepads, are HID devices. Typically they contain a single interrupt endpoint with a 8 B or 64 B packet size. The keys pressed, mouse movement, joysticks, etc are all encoded in these bytes. There is an HID report which tells you how to interpret the bytes to know which keys, mouse input, or gamepad input it means. An eHCI driver needs to focus on receiving the interrupt packets.

The endpoint descriptor describes the packet size and the (maximum supported) interrupt interval. The maximum supported interrupt frequency for USB 1 is an interval of 1ms (aka 1 frame). The maximum supported frequency for USB 2 devices is an interval of 125 micro-seconds (aka 1 microframe). Create a QH to do transfers with the interrupt endpoint. Create 1 or more IN QTDs to transfer the data, and create a buffer of <packet size> per QTD for each QTD to point to. You need at least 1 QTD. Having more QTDs lets you buffer more packets, and reduce the chances of your OS missing a packet if it doesn't process completed QTDs in time before the next packet. Add the QH to the periodic frame list. Based on the interval, add your QTD accordingly in the periodic list. Use the S-mask field to specify which microframes within a frame slot your QH will be executed in. If there is space in the periodic list, match the interval exactly. Polling too frequently will just result in NACKs and will not improve gaming. You can choose to make the interval smaller than the maximum supported interval, but this will be a degraded user experience with higher latency.

## Async vs periodic lists
The async list is for non-latency-sensitive transfers, which are control transfers (adjusting settings, small amounts of data) and bulk transfers (used by USB mass storage devices, which you want high throughput but don't want messing with your keyboard, mouse, and microphone input). These are transfers that you want to get done as soon as possible, and are flexible.

The periodic list is for interrupt transfers (short, frequent packets from devices like keyboards and mice) and isochronous transfers (higher throughput and latency-sensitive applications such as microphones and speakers). These are transfers which you want to do *periodically*, at a constant data rate at a constant interval.

The eHCI measures time in 1ms "frames". Each frame is made up of 8 microframes (a concept introduced in USB 2.0), which as 125 microseconds each. In each microframe, the eHCI first does all of the transfers from the periodic list scheduled for that microframe, and then uses the remaining time to do transfers from the async list. The USB protocol requires 20% of the time to be reserved for the async list, so the periodic list can take up a maximum of 80% of the bandwidth if there are transfers waiting in the async list.

When writing a driver the first type of transfer you will do is a control transfer on endpoint 0. This means that the first data structure you will use is the async list. The first time you will use the periodic list will probably be for getting USB keyboard/mouse input.

## Doing control transfers with the async list
The async list is a round robin singularly circular linked list of queue heads. Each Queue Head (QH) represents communication with a certain endpoint of a device. Each Queue Head contains a singularly linked list of Queue Transfer Descriptors (QTDs). 

A QTD descripes a phase of communication with the endpoint, which can be SETUP, IN, or OUT, and has a pointer to a buffer associated with it if there is data being transferred. For IN and OUT transfers, a QTD can point to a buffer of up to 20 KiB. It is important not to confuse a QTD and a packet, even though for many transfers 1 QTD is completed in 1 packet. For example, a QTD can transfer 20 KiB of data, even if the max packet size is 64 B. The eHCI will work on the QTD and update its status as it makes progress. If you need to transfer more than 20 KiB, you can simply chain multiple consecutive QTDs.

You initialize the async list by creating the first Queue Head and giving the eHCI a pointer to it. Also, one Queue Head in the circular list must have the H-bit set, which is used by the eHCI to know when it has traversed the entire list. Going from 1 Queue Head to 0 Queue Heads in the async list is complicated. If at any point you want to have an empty async list, I recommend creating a dummy / placeholder Queue Head with the H-bit set, which you can use to initialize the async list.

To add a Queue Head to the list, simply choose an existing Queue Head in the list, make it point to the new Queue Head, and make the new Queue Head point to the next Queue Head in the list.

To remove a Queue Head from the list, remove the pointer to the Queue Head you want to remove from the previous QH, and make it point to the next QH you want to keep. Make sure the QH you want to remove is still valid and points to a valid QH. It can be helpful to store a pointer to the previous QH so you can find the previous QH. Ring the eHCI's doorbell by writing a 1 in USBCMD. Then wait for an async advance interrupt. Once that happens the process of freeing the QH is complete. You can remove multiple QHs from the list and then ring the doorbell to remove them in batches. However, if you already rang the doorbell and are waiting for an async advance interrupt and you want to remove additional QHs, then you will have to wait until the pending async advance interrupt, re-ring the doorbell, and then wait for another async advance interrupt.

## Doing interrupt transfers with the periodic list
The periodic list gives your OS a lot of control over the exact time slots. Each frame has a literal slot in memory corresponding to it. Each slot contains a singularly linked list of Queue Heads. In each microframe, the eHCI will go through this list of QHs. If the first QH has a QTD to execute, it will be executed. Then, the next QH will be processed, and the next one. A microframe is only 125 microseconds and if you make this list of QHs too long, the eHCI may not be able to process them. For interrupt transfers this won't be an issue since they take up so little time. Each QH contiains a singularly linked list of QTDs. Each QTD corresponds to a single packet. Once a QTD is processed, the next QTD won't be processed until the next microframe that the QH is a part of.

Your initial periodic list will be empty. Each slot will indicate that it is not a valid pointer (basically a `None`). As you add USB HID devices, you will assign different slots to different devices. Each slot represents a frame (1 ms). However, the eHCI works in microframes, not frames. You can specify which microframe a periodic transfer will be run in based on the S-mask. Note that USB 1.x devices only know frames and have no concept of microframes. And in practice, most HID devices will talk in USB 1.x. This is more complex than high-speed interrupt endpoints, since the eHCI has to go through a hub to talk to the low or full speed device (TODO, include how to talk to USB 1.x devices). In QEMU, keyboards and mice are high speed devices if you attach them to the eHCI bus, so you can start with this easy case.

Let's say you have a QEMU USB keyboard, which has a packet size of 8 B and interval of 8 ms. You only use a single QH for this keyboard, but you reference the QH within multiple slots. Since the interval is 8 ms, you point to the QH every 8 slots in the periodic list. Since the list has 1024 slots, you will make 128 of those slots point to the QH. You can choose slots 0, 8, 16, etc. You can also choose 1, 9, 17, etc. It's a good idea to see which slots already have transfers and stagger the transfers.

Within the QH you can actually make a circular linked list of QTDs, like a ring buffer for the eHCI to transfer data to your OS. Enable the IOC (Interrupt on Complete) bits of each QTD, and when you process the completed QTD, reset its buffer and token to mark it as ready to transfer again when the eHCI loops back around. If the device responds with a NACK, the eHCI will treat the QTD as still not completed and will not issue an interrupt. For HID devices, most packets will be a NACK, and your OS will conveniently only have to process packets that contain actual HID input. If your OS does not process the packets in time and the ring of QTDs are all full, the eHCI will see that the next QTD does not have the Active bit set and will not advance to the next QTD. Once your OS resets the QTD, the eHCI will execute the QTD the next time the QH is processed. 

### Frame list size
Depending on the eHCI, there are either 1024 slots (meaning that it takes 1024 ms to go through the entire list), or, if supported, you can choose if you want 1024, 512, or 256 slots. Choosing 256 slots as the size is really for computers where 3 KiB of RAM usage (how much you save by selecting 256 slots) savings matters. You are probably not in such as resource-constrained environment. So just don't care if smaller frame list sizes are supported, and stick to 1024, which will always be supported.
