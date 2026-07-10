use samp::{
    amx::Allocator,
    args::Args,
    cell::{Ref, UnsizedBuffer},
    error::AmxError,
    prelude::AmxString,
};
use snafu::{ensure, OptionExt, ResultExt};
use std::convert::TryFrom;
use std::num::TryFromIntError;

/// These are the types of arguments the plugin supports for passing on to the callback.
#[derive(Debug, Clone)]
pub enum PassedArgument {
    PrimitiveCell(i32),
    Str(Vec<u8>),
    Array(Vec<i32>),
}

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
    #[snafu(display("Array length points outside the script's memory"))]
    ArrayOutOfBounds,
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

    /// Consumes variadic PAWN params into Vec<PassedArgument>.
    /// It expects `args` to be positioned at a string of type letters,
    /// e.g. `"iiaAs"` ("two integers, an array and its length, and a string"),
    /// which instructs us how to interpret the variadic arguments that follow.
    pub fn from_amx_args(mut args: Args) -> Result<VariadicAmxArguments, ArgError> {
        let letters = args.next_arg::<AmxString>().context(MissingTypeLetters)?.to_bytes();
        let mut args = args.into_variadic();
        let (expected, received) = (letters.len(), args.remaining());
        ensure!(expected == received, MismatchedAmountOfArgs { received, expected, letters });

        let mut letters = letters.into_iter();
        let mut collected_arguments: Vec<PassedArgument> = Vec::with_capacity(expected);
        while let Some(type_letter) = letters.next() {
            collected_arguments.push(match type_letter {
                b's' => PassedArgument::Str(args.next_arg::<AmxString>().context(MissingArg)?.to_bytes()),
                b'a' => PassedArgument::Array({
                    ensure!(matches!(letters.next(), Some(b'i' | b'A')), MissingArrayLength);
                    let buffer: UnsizedBuffer = args.next_arg().context(MissingArg)?;
                    let length_ref: Ref<i32> = args.next_arg().context(MissingArg)?;
                    let length = usize::try_from(length_ref.get()).context(InvalidArrayLength)?;
                    buffer.into_sized_buffer(length).ok().context(ArrayOutOfBounds)?.to_vec()
                }),
                _ => PassedArgument::PrimitiveCell(args.next_arg::<Ref<i32>>().context(MissingArg)?.get()),
            });
        }
        Ok(Self {
            inner: collected_arguments,
        })
    }

    /// Push the arguments onto the AMX stack, in first-in-last-out order, i.e. reversed.
    /// Strings and arrays are allotted on the AMX heap; the allocator (and
    /// with it, the heap frame) is kept alive by the caller until the
    /// callback has executed.
    pub fn push_onto_amx_stack(&self, allocator: &Allocator) -> Result<(), AmxError> {
        let amx = allocator.amx();
        for param in self.inner.iter().rev() {
            match param {
                PassedArgument::PrimitiveCell(cell_value) => {
                    amx.push(cell_value)?;
                }
                PassedArgument::Str(bytes) => {
                    amx.push(allocator.allot_bytes(bytes)?)?;
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
