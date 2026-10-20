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

# Design Choices
## Key structs
- QH with metadata (128 B)
  - Actual QH is 96 B (128 B rounded to its align of 32 B)
  - Without 64-bit support this is 48 B (64 B rounded to its align of 32 B)
  - A QH without 64-bit support but with meta-data would be 64 B
  - May be worth having a variant without 64-bit support?
- QTD (64 B)
  - QTD without 64-bit support is 32 B
  - QTD with 64-bit support is 52 B, so we could use those 12 B of padding to store something if we want
- Periodic frame list (4096 B)
## Things to consider
- I am new to writing drivers. I started writing an xHCI driver based on [Stellux OS's Tutorial](https://www.youtube.com/watch?v=W0EqQiDSoxE)
- My "OS" is currently a kernel with a single thread which runs on a single core, with an async executor
- I want to first learn about USB and how to get eHCI working before focusing on performance and integrating it into a "real" OS
- Compatibility with non-Rust programming languages is not a goal
- Code that would use this library is probably
  - A unikernel
  - Written in Rust
  - Uses async itself already
  - Has enough memory
  - Doesn't care much about performance (ok to allocate frequently and allocate way more than necessary)

## Why async?
- Interrupt based, and async can work pretty decently in unikernels when you need to wait until there is an interrupt.
- Lets you write unikernels with a big async function without thinking about things too much, and focus on the fun.
- Async code can feel really natural

## Interrupts and allocations
- The IRQ handler
  - Interrupts assumed to be disabled (it is extremely normal for interrupt handlers to always run with interrupts disabled)
  - Can allocate? Maybe, depends on the OS. Many Rust hobby unikernels have a global allocator based on a spin lock. If the IRQ handler preempts something that was holding the lock, then it would cause a deadlock if the IRQ handler tried to allocate. So if our IRQ handler did allocate, then we would have to have special instructions for the OS to make sure allocations are possible when calling our IRQ handler. This would most likely involve making the allocator always disable interrupts before acquiring the lock. I want to avoid making operating systems do this, since it shifts this extra complexity on the OS.
  - Can run on multiple cores? Practically, what is the point of this? eHCI doesn't support MSI and fancy interrupt routing like xHCI does. It routes to a I/O APIC irq, which routes to a LAPIC. We should make the IRQ handler be able to be called from *any* core, but require the OS to not have simultaneous calls to the IRQ handler (that wouldn't even make sense).
- Regular fns
  - Run with interrupts enabled? Depends on the OS. If we require interrupts to be disabled while calling our functions and calling `poll` on our futures, that might make things easier for locks used by normal fns and the IRQ handler.  However, this adds an extra restriction to the OS.
  - Can allocate? Yes. This is not unreasonable to ask the OS developer. Using `alloc` is very common and we will use it heavily. We will `Arc` without thinking "Hmm can this be an `T<'a>`?" and things turning into `struct MyStruct<'a, 'b, 'c>`. So yes, we will require allocations to be possible.
  - Can run on multiple cores? Yes, I think many unikernels will implement SMP and utilize multiple cores and move futures around. We should make sure that all of our structs are `Send` and `Sync` and support concurrency. It should also run just fine on a single-core system.

## Challenge: forwarding interrupts to multiple wakers
One of the most important and highly needed eHCI interrupt is the "USB interrupt", which will fire when any transfer is complete. On a busy system that's doing multiple USB things, it will constantly be getting eHCI interrupts. eHCI doesn't tell us *which* QTD was complete, so the only way to know is by reading the QTDs.

### Minimal Interrupt Sources
- Laptop with eHCI + companion controller, no USB devices plugged in
- Nothing would cause USB Interrupts, only port change interrupt on device plug
- Only a single "subscriber" to the port change interrupt

### Realistic Example
- You have an OS with 1-2 HID devices (for example, keyboard + mouse or gamepad) running on a laptop with Integrated Rate Matching Hubs
- Root port status change interrupt
- 1 periodic endpoint for hub events
- 1 periodic endpoint for one HID device
- 1 periodic endpoint for the second HID device, if it's connected

### More complex but still realistic example
- eHCI root hub
  - port status change int
- Integrated Rate Matching Hub
  - interrupt endpoint
- 2 physical
  - 2 interrupt endpoints
- keyboard, mouse, and 4 gamepads
  - 6 interrupt endpoints
- 1 mic and 1 speaker
  - 2 isochronous endpoints

### Theoretical max
127 devices, each with 32 endpoints, with a total of 4064 endpoints!

### What makes it challenging
Because our library is async, the pattern is that we have 2 main parts to our API:
- Fun stuff (actually interact with hubs, devices, and endpoints)
- IRQ handler (required, of course, since this is an interrupt and not polling based driver)

The fun stuff returns futures which will get a waker, and then will need to store that waker somewhere. Then the IRQ handler will wake the waker so that the OS's async executor will poll the futures which could've became ready because of the IRQ.

We will have >1 waker associated with an IRQ, especially the USB int IRQ. We might even have hundreds of wakers! So, in our IRQ handler do we call every waker? And how do we store an unbounded number of wakers? What data structure do we use and how does it affect allocations?

### Possible Solution: waker list where IRQ handler never has to deal with contention
The list can be an atmoic `Option<Box<Collection<T>>>` where `Collection` can be a `Vec`, `BTreeMap`, etc. The IRQ handler can just atomic swap it to `None`, indicating that it is locked by the IRQ handler. It can then call all of the wakers. The waker must not allocate or deallocate (not that hard to meet this requirement). Then the IRQ handler can just atomic store it again. This way the IRQ handler doesn't have to deallocate wakers. That can be implemented in the `Drop` trait of the future that added the waker in the first place. 

For futures, there will be a separate Mutex they have to go through to add/remove wakers from the list. We could just make this a spin mutex. They will only need to spin in a multi-core system, so this shouldn't cause issues with most OSs using this library. We could also use `lock_api` to let multi-core operating systems use an IPI based mutex. Once they acquire the lock, they have to unsafely get an `&Box`, copy it to make a new `Box`, then compare and exchange the old box with the new box atomically. This way, the IRQ handler can get an `&Box` at any time. If the IRQ handler swaps it to a `None`, then the compare and exchange will fail, and the future will have to spin until the IRQ handler releases the box again. This makes sure that when the IRQ handler swaps back the `Box`, it will always get back a `None` and not a `Box`. 

This can be called `RoyalMutex`. The IRQ handler is "royal" and never has to wait. Everything else will wait for it. Everything else has to share with each other (could be backed by an unfair mutex or a fair mutex) and always has less priority than the IRQ handler. Only 1 thread can be "royal" at a time. This can be enforced by a runtime panic if a `royal_lock` swaps with `None` and then receives a `None`. 

### Another possible solution: the IRQ handler doesn't actually need to obtain the mutex
There is nothing special about the IRQ handler. It's just a signal from the eHCI, "there is new data to be processed". The IRQ handler just needs to make sure that the data is processed, which is simply reading USBSTS and then processing any updates based on its value. The eHCI handler can attempt to acquire the lock to global data. We can call this global data lock. If it doesn't acquire the lock, it will leave a flag for whatever releases the lock to first process any updates. The IRQ handler can say "you're holding the lock? that's fine, just do my work for me before you release it. i'm going to return.". This way, we don't actually need a royal mutex. We can just have an `AtomicBool` which is an indicator than an IRQ occurred, along with a regular mutex (spin or generic). Other futures will still have to acquire the mutex because they need to insert/remove wakers (again, there will only be contention if )

## Support sync Rust mode?
Use case: maybe you're kernel logger is going to log through a USB to UART adapter. Enabling interrupt while logging messages adds complexity and can make things hard to debug. We could still keep this library sync while making sure it works in a spin loop async executor.

But then, what about the IRQ handler? If interrupt are disabled, our IRQ handler won't be called during this synchronous execution. Is this OK? This means that we can't rely on the interrupt handler doing core logic. The interrupt handler's job will basically be to wake a waker on an interrupt. But this means that other structs which there can be multiple copies of might try to access common registers such as USBCMD and USBSTS concurrently (if there are multiple cores). 

USBSTS should be fine. Everything can be done atomically. You can read the u32. The only writes are to clear bits, and simultaneous writes won't interfere with each other. 

USBCMD has a bunch of RW fields. Updating a single fields requires a read-modify-write. So we need to make sure that only 1 piece of code has an `&mut` to USBCMD.

One way we can solve this is by having a driver future. The driver future will own `&mut` USBCMD (and other configuration related things). This will make other futures depend on the driver future being polled for them to work properly.

What if an OS does this:
- async executor
- join
  - driver future
  - async fn 
    - log::info!()
    - logger
    - need to log to the USB to UART device somehow. we can't just spin poll the fns on the USB device struct because that would starve the driver future, causing nothing to happen.

What if instead every single async fn *could* drive itself? Then this library could support sync loggers which temporarily spin loop poll certain USB operations. This would mean:
- Every async function has to check USBSTS and possibly modify USBCMD every time it is polled.
- We would need to put USBCMD and more in a Mutex or RwLock. In a single-core system with 1 thread, there would never be any contention on the lock. In a multi-core system with 1 thread per core, there might be contention but there won't be a deadlock.
- We have to be careful to not attempt to acquire the lock in the interrupt handler since that might deadlock if `poll` is called with interrupts enabled.
- We can make interrupt handler take an `&self` instead of `&mut self` because things would be behind locks anyways.

## What does a without interrupts async executor look like?
```rs
pub fn execute_future<F: Future>(future: F) -> F::Output {
    let mut pinned = pin!(future);
    let waker = Waker::from(noop);
    let mut cx = Context::from_waker(&waker);
    loop {
        match pinned.poll_unpin(&mut cx) {
            Poll::Ready(output) => break output,
            Poll::Pending => {}
        }
    }
}
```
`execute_future` can be problematic if called inside an async function.

## What does a single-core async executor look like?
- disable interrupts
- poll with a noop waker
- if ready, done!
- if pending, enable and hlt, and then poll again

## What does a multi-core async executor look like?
Idk how many unikernels are going to have a multi-core executor while still only having 1 thread per CPU. Maybe once you have the complexity of 4 kernel threads you might add things like multiple kernel threads on 1 CPU, preemption, maybe user mode? I don't see much of a point to this since a big reason why a simple rust kernel will have an async executor is to run everything on one core and not deal with multiple threads. Async fns should be targeting 0% CPU usage anyways. Realistically 0.1-10% is the target. There isn't much benfit to splitting 10% of a CPU of work into 4 different CPUs.

- waker that sends an IPI to all other cores (this way, the waker can be sent across cores and it can still wake whatever future needs to be woken even if that future is on a different core)
- disable interrupts
- poll with the waker
- if ready, done (exit loop)
- if pending, enable and hlt (IRQs processed on other cores that call wakers will wake up this core from hlt), and then poll again.

## Use case: synchronous loggers
- Log to a USB to UART device
- Do it synchronously
- Call logger from inside an async fn
- Handle disconnects and reconnects of the USB to UART device in the middle of a log?

## Struct heirarchy
Goals:
- Use `'static` lifetimes so that everything can be sent around and stored freely
- Ok to use runtime borrow checking and heavy use of `Arc`

- NewDeviceStream struct for getting new devices?
- Hub struct for manual control over hubs?
- Device (can have different stages of initialization each have their own type). A device will have a configuration set. This driver will remember the configurations.
- Interface that owns
