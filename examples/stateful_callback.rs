// Drive acquisition from a callback that mutates shared outer state, instead
// of a polling loop.
//
// openDAQ invokes a reader's "data available" procedure from its own scheduler
// thread(s) as packets arrive.  A Rust closure handed to openDAQ must therefore
// be `Send + Sync`, and any state it touches has to be shared safely -- an
// `Arc` around atomics or a `Mutex`, never a plain captured `&mut`, an `Rc`, or
// a `RefCell` (those wouldn't even compile here, precisely because the callback
// can run on another thread).
//
// The callback below tallies -- into outer state shared with `main` -- how many
// times openDAQ signalled that new samples were ready, and which threads it was
// called from.  The main thread reads the samples and, afterwards, the
// accumulated state back.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use opendaq::{Channel, Instance, Procedure, StreamReader};

fn main() -> opendaq::Result<()> {
    let instance = Instance::new()?;
    let device = instance.add_device("daqref://device0")?.expect("device");
    let channel = instance
        .find_component("Dev/RefDev0/IO/AI/RefCh0")?
        .expect("reference channel not found")
        .cast::<Channel>()?;
    device.set_property_value("GlobalSampleRate", 1000)?;
    let signal = &channel.signals()?[0];

    let reader = StreamReader::<f64>::new(signal)?;

    // The outer state the callback mutates, shared with the main thread: an
    // atomic counter of notifications, and the set of thread ids we were
    // called from.  Both live behind `Arc` so the closure and `main` share
    // one instance.
    let notifications = Arc::new(AtomicUsize::new(0));
    let caller_threads = Arc::new(Mutex::new(BTreeSet::<String>::new()));

    // Clone the Arc handles into the closure.  It captures only these (not the
    // reader), so there is no ownership cycle, and it is `move` + Send + Sync.
    let notifications_cb = Arc::clone(&notifications);
    let caller_threads_cb = Arc::clone(&caller_threads);
    let on_ready = Procedure::from_fn(move |_args| {
        notifications_cb.fetch_add(1, Ordering::Relaxed);
        let thread = format!("{:?}", std::thread::current().id());
        caller_threads_cb.lock().unwrap().insert(thread);
        Ok(())
    })?;
    reader.set_on_data_available(&on_ready)?;

    // Let the simulator stream for a moment.  The callback fires in the
    // background as packets arrive; here on the main thread we drain the
    // samples and keep a running total to show acquisition really is flowing.
    let mut total_samples = 0usize;
    let mut running_sum = 0.0f64;
    for _ in 0..10 {
        let samples = reader.read(1000, 200)?;
        total_samples += samples.sample_count();
        running_sum += samples.iter().sum::<f64>();
        std::thread::sleep(Duration::from_millis(50));
    }

    // Read the accumulated outer state back on the main thread.
    let fired = notifications.load(Ordering::Relaxed);
    let caller_threads = caller_threads.lock().unwrap();
    let main_thread = format!("{:?}", std::thread::current().id());
    let mean = if total_samples > 0 {
        running_sum / total_samples as f64
    } else {
        0.0
    };

    println!("Read {total_samples} samples (mean {mean:.4}).");
    println!("The data-ready callback fired {fired} time(s) while we read.");
    println!("Main thread: {main_thread}");
    println!("Callback ran on:  {caller_threads:?}");
    if caller_threads.iter().any(|t| *t != main_thread) {
        println!(
            "The callback ran on an openDAQ scheduler thread -- which is why its \
             closure must be Send + Sync and its state shared through Arc/atomics."
        );
    } else {
        println!(
            "openDAQ happened to notify us on the calling thread this run, but it is \
             free to use its own worker threads, so the Send + Sync / shared-state \
             contract still applies."
        );
    }

    Ok(())
}
