// Calling function properties: properties whose value is a callable object.
//
// The reference device exposes "Sum" and "SumList" function properties nested
// under its "Protected" object property.  Reading such a property yields a
// callable [`opendaq::FunctionObject`] to invoke with boxed arguments.

use opendaq::{Device, FunctionObject, Instance, Value};

/// The value of a FUNC-typed property, as the callable it is.
fn function_property(device: &Device, name: &str) -> opendaq::Result<FunctionObject> {
    device
        .property_value(name)?
        .into_object()?
        .cast::<FunctionObject>()
}

fn main() -> opendaq::Result<()> {
    let instance = Instance::new()?;
    let device = instance.add_device("daqref://device0")?.expect("device");

    let sum = function_property(&device, "Protected.Sum")?;
    println!(
        "Protected.Sum(7, 5)   = {}",
        sum.call(&[Value::from(7), Value::from(5)])?
    );
    println!(
        "Protected.Sum(40, 2)  = {}",
        sum.call(&[Value::from(40), Value::from(2)])?
    );
    println!(
        "Protected.Sum(100, 1) = {}",
        function_property(&device, "Protected.Sum")?.call(&[Value::from(100), Value::from(1)])?
    );

    // SumList takes a single argument: a list of numbers.
    let sum_list = function_property(&device, "Protected.SumList")?;
    println!(
        "Protected.SumList([1, 2, 3, 4]) = {}",
        sum_list.call(&[Value::from(vec![1, 2, 3, 4])])?
    );
    println!(
        "Protected.SumList([]) = {}",
        sum_list.call(&[Value::List(Vec::new())])?
    );

    // `call` passes arguments straight through, and an implementation is free
    // to ignore extras (the reference device's Sum does).  The declared
    // signature lives in the property's callable info -- check the arity
    // there before calling when it matters.
    let info = device
        .property("Protected.Sum")?
        .expect("Sum property")
        .callable_info()?
        .expect("callable info");
    let expected = info.arguments()?.len();
    let args = [Value::from(1), Value::from(2), Value::from(3)];
    if args.len() == expected {
        println!("\nProtected.Sum(1, 2, 3) = {}", sum.call(&args)?);
    } else {
        println!(
            "\nWrong arity is rejected: Protected.Sum expects {expected} arguments, got {}.",
            args.len()
        );
    }
    Ok(())
}
