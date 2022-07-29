use log::error;
use std::convert::TryFrom;
use samp::prelude::AmxString;

/// These are the types of arguments the plugin supports for passing on to the callback.
#[derive(Debug, Clone)]
pub enum PassedArgument {
    PrimitiveCell(i32),
    Str(Vec<u8>),
    Array(Vec<i32>),
}

/// Consumes variadic PAWN params into Vec<PassedArgument>
/// It expects the first of args to be a string of type letters, i.e. "dds",
/// which instruct us how to interpret the following arguments.
pub fn parse_variadic_arguments_passed_into_timer(
    mut args: samp::args::Args,
) -> Option<Vec<PassedArgument>> {
    let argument_type_letters = args.next::<AmxString>()?.to_bytes();
    if argument_type_letters.len() != args.count() - 4 {
        error!("The amount of callback arguments passed ({}) does not match the length of the list of types ({}).",args.count() - 4, argument_type_letters.len());
        return None;
    }

    let mut collected_arguments: Vec<PassedArgument> = Vec::with_capacity(argument_type_letters.len());
    let mut argument_type_letters = argument_type_letters.iter();

    while let Some(type_letter) = argument_type_letters.next() {
        collected_arguments.push(
            match type_letter {
                b's' => PassedArgument::Str( args.next::<AmxString>()?.to_bytes() ),
                b'a' => {
                    if let Some(b'i') | Some(b'A') = argument_type_letters.next() {
                        let array_argument: samp::cell::UnsizedBuffer = args.next()?;
                        let length_argument = args.next::<i32>().and_then(|len| usize::try_from(len).ok())?;
                        let amx_buffer = array_argument.into_sized_buffer(length_argument);

                        PassedArgument::Array( amx_buffer.as_slice().to_vec() )
                    } else {
                        error!("Array arguments (a) must be followed by an array length argument (i/A).");
                        return None;
                    }
                },
                _ => PassedArgument::PrimitiveCell( args.next::<i32>()? )
            }
        );
    }
    Some(collected_arguments)
}