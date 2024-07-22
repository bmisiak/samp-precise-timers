use samp::{
    amx::{Allocator, Amx},
    args::Args,
    cell::UnsizedBuffer,
    consts::AmxExecIdx,
    error::AmxError,
    prelude::AmxString,
};
use snafu::{ensure, OptionExt, ResultExt};
use std::{convert::TryInto, num::TryFromIntError, pin::Pin};

/// These are the types of arguments the plugin supports for passing on to the callback.
#[derive(Debug, Clone)]
pub enum PassedArgument {
    PrimitiveCell(i32),
    Str(Vec<u8>),
    Array(Vec<i32>),
}

/// A callback which MUST be executed.
/// Its args are already on the AMX stack.
#[ouroboros::self_referencing]
pub(crate) struct StackedCallback {
    pub amx: Amx,
    #[borrows(amx)]
    #[covariant]
    pub allocator: Allocator<'this>,
    pub callback_idx: AmxExecIdx,
}
/*
impl StackedCallback {
    /// ### SAFETY:
    /// The `amx.exec()` here might call one of our natives
    /// such as `SetPreciseTimer` (`PreciseTimers::create`).
    /// Those will try to acquire mutable access to
    /// the scheduling store(s) e.g. `TIMERS` and `QUEUE`.
    /// To avoid aliasing, there MUST NOT be any
    /// active references to them when this is called.
    #[inline]
    #[must_use]
    pub unsafe fn execute(self) -> Result<i32, AmxError> {
        self.amx.exec(self.callback_idx)
    }
}*/

#[derive(Debug, Clone)]
pub(crate) struct VariadicAmxArguments {
    inner: Vec<PassedArgument>,
}

#[derive(Debug, snafu::Snafu)]
#[snafu(context(suffix(false)))]
pub(crate) enum ArgError {
    #[snafu(display("The list of types ({letters:?}) has {expected} letters, but received {received} arguments."))]
    MismatchedAmountOfArgs {
        received: usize,
        expected: usize,
        letters: Vec<u8>,
    },
    MissingTypeLetters,
    MissingArrayLength,
    MissingArg,
    InvalidArrayLength {
        source: TryFromIntError,
    },
}

impl From<ArgError> for AmxError {
    fn from(value: ArgError) -> Self {
        log::error!("param error: {value:?}");
        AmxError::Params
    }
}

#[rustfmt::skip]
impl VariadicAmxArguments {
    #[cfg(test)]
    pub fn empty() -> Self {
        Self { inner: vec![] }
    }

    /// The user of the plugin specifies what kind of arguments
    /// they want passed onto the timer, followed by the
    /// actual arguments.
    //// This verifies the validity of the letters.
    /// # Example
    /// `"iiaAs"`: "two integers, an array and its length, and a string"
    fn get_type_letters<const SKIPPED_ARGS: usize>(
        args: &mut Args,
    ) -> Result<impl ExactSizeIterator<Item = u8>, ArgError> {
        let non_variadic_args = SKIPPED_ARGS + 1;
        let letters = args.next::<AmxString>().context(MissingTypeLetters)?.to_bytes();
        let expected = letters.len();
        let received = args.count() - non_variadic_args;
        ensure!(expected == received, MismatchedAmountOfArgs { expected, received, letters });
        Ok(letters.into_iter())
    }

    /// Consumes variadic PAWN params into Vec<PassedArgument>
    /// It expects the first of `args` to be a string of type letters, e.g. `"dds"`,
    /// which instruct us how to interpret the following arguments.
    pub fn from_amx_args<const SKIPPED_ARGS: usize>(
        mut args: Args,
    ) -> Result<VariadicAmxArguments, ArgError> {
        let mut letters = Self::get_type_letters::<SKIPPED_ARGS>(&mut args)?;
        let mut collected_arguments: Vec<PassedArgument> = Vec::with_capacity(letters.len());

        while let Some(type_letter) = letters.next() {
            collected_arguments.push(match type_letter {
                b's' => PassedArgument::Str(args.next::<AmxString>().context(MissingArg)?.to_bytes()),
                b'a' => PassedArgument::Array({
                    ensure!(matches!(letters.next(), Some(b'i' | b'A')), MissingArrayLength);
                    let buffer: UnsizedBuffer = args.next().context(MissingArg)?;
                    let length = args.next::<i32>().context(MissingArg)?.try_into().context(InvalidArrayLength)?;
                    let sized_buffer = buffer.into_sized_buffer(length);
                    sized_buffer.as_slice().to_vec()
                }),
                _ => PassedArgument::PrimitiveCell(args.next::<i32>().context(MissingArg)?),
            });
        }
        Ok(Self {
            inner: collected_arguments,
        })
    }

    /// Push the arguments onto the AMX stack, in first-in-last-out order, i.e. reversed
    pub fn push_onto_amx_stack<'cb, 'amx: 'cb>(
        &self,
        amx: Amx,
        callback_idx: AmxExecIdx,
    ) -> Result<StackedCallback, AmxError> {

        Ok(StackedCallbackBuilder {
            amx: amx.clone(),
            callback_idx, 
            allocator_builder: |amx| { 
                let allocator = amx.allocator();
                for param in self.inner.iter().rev() {
                    match param {
                        PassedArgument::PrimitiveCell(cell_value) => {
                            amx.push(cell_value).unwrap();
                        }
                        PassedArgument::Str(bytes) => {
                            let buffer = allocator.allot_buffer(bytes.len() + 1).unwrap();
                            let amx_str = unsafe { AmxString::new(buffer, bytes) };
                            amx.push(amx_str).unwrap();
                        }
                        PassedArgument::Array(array_cells) => {
                            let amx_buffer = allocator.allot_array(array_cells.as_slice()).unwrap();
                            amx.push(array_cells.len()).unwrap();
                            amx.push(amx_buffer).unwrap();
                        }
                    }
                }
                allocator
            }
        }.build())
    }
}
