use log::error;
use samp::{error::AmxError, prelude::AmxString};
use std::convert::TryFrom;

/// These are the types of arguments the plugin supports for passing on to the callback.
#[derive(Debug, Clone)]
pub enum PassedArgument {
    PrimitiveCell(i32),
    Str(Vec<u8>),
    Array(Vec<i32>),
}

#[derive(Debug, Clone)]
pub struct VariadicAmxArguments {
    inner: Vec<PassedArgument>,
}

impl VariadicAmxArguments {
    fn get_type_letters(
        args: &mut samp::args::Args,
        skipped_args: usize,
    ) -> Result<Vec<u8>, AmxError> {
        let non_variadic_args = skipped_args + 1;
        let variadic_argument_types = args.next::<AmxString>().ok_or_else(|| {
            error!("Missing an argument which specifies types of variadic arguments.");
            AmxError::Params
        })?;
        let type_letters = variadic_argument_types.to_bytes();
        let expected_variadic_args = type_letters.len();
        let received_variadic_args = args.count() - non_variadic_args;
        
        if expected_variadic_args == received_variadic_args {
            Ok(type_letters)
        } else {
            error!("The amount of arguments passed ({}) does not match the length of the list of types ({}: {}).",
                received_variadic_args,
                expected_variadic_args,
                variadic_argument_types
            );
            Err(AmxError::Params)
        }
    }

    /// Consumes variadic PAWN params into Vec<PassedArgument>
    /// It expects the first of args to be a string of type letters, e.g. `"dds"`,
    /// which instruct us how to interpret the following arguments.
    pub fn from_amx_args(
        mut args: samp::args::Args,
        skipped_args: usize,
    ) -> Result<VariadicAmxArguments, AmxError> {
        let type_letters = Self::get_type_letters(&mut args, skipped_args)?;

        let mut collected_arguments: Vec<PassedArgument> = Vec::with_capacity(type_letters.len());
        let mut argument_type_letters = type_letters.iter();

        while let Some(type_letter) = argument_type_letters.next() {
            collected_arguments.push(match type_letter {
                b's' => PassedArgument::Str(args.next::<AmxString>().ok_or(AmxError::Params)?.to_bytes()),
                b'a' => {
                    if let Some(b'i' | b'A') = argument_type_letters.next() {
                        let array_argument: samp::cell::UnsizedBuffer = args.next().ok_or(AmxError::Params)?;
                        let length_argument = args
                            .next::<i32>()
                            .and_then(|len| usize::try_from(len).ok()).ok_or(AmxError::Params)?;
                        let amx_buffer = array_argument.into_sized_buffer(length_argument);

                        PassedArgument::Array(amx_buffer.as_slice().to_vec())
                    } else {
                        error!(
                            "Array arguments (a) must be followed by an array length argument (i/A)."
                        );
                        return Err(AmxError::Params);
                    }
                }
                _ => PassedArgument::PrimitiveCell(args.next::<i32>().ok_or(AmxError::Params)?),
            });
        }
        Ok(Self {
            inner: collected_arguments,
        })
    }

    /// Push the arguments onto the AMX stack, in first-in-last-out order, i.e. reversed
    pub fn push_onto_amx_stack(
        &self,
        amx: &samp::amx::Amx,
        allocator: samp::amx::Allocator,
    ) -> Result<(), AmxError> {
        for param in self.inner.iter().rev() {
            match param {
                PassedArgument::PrimitiveCell(cell_value) => {
                    amx.push(cell_value)?;
                }
                PassedArgument::Str(bytes) => {
                    let buffer = allocator.allot_buffer(bytes.len() + 1)?;
                    let amx_str = unsafe { AmxString::new(buffer, bytes) };
                    amx.push(amx_str)?;
                }
                PassedArgument::Array(array_cells) => {
                    let amx_buffer = allocator.allot_array(array_cells.as_slice())?;
                    amx.push(array_cells.len())?;
                    amx.push(amx_buffer)?;
                }
            }
        }
        Ok(())
    }
}
