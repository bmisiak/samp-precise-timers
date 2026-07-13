use samp::{
    amx::Allocator,
    args::VariadicArgs,
    cell::{RawCell, Ref, UnsizedBuffer},
    error::AmxError,
    prelude::AmxString,
};

/// These are the types of arguments the plugin supports for passing on to
/// the callback. Cell values are kept as uninterpreted [`RawCell`]s: we only
/// round-trip them back onto the AMX stack, so `Float:`/`bool:` tags don't
/// matter.
#[derive(Debug, Clone)]
pub enum PassedArgument {
    PrimitiveCell(RawCell),
    Str(Vec<u8>),
    Array(Vec<RawCell>),
}

#[derive(Debug, Clone)]
pub(crate) struct VariadicAmxArguments {
    inner: Vec<PassedArgument>,
}

custom_error::custom_error! {
    pub(crate) ArgError
    MismatchedAmountOfArgs { received: usize, expected: usize, letters: Vec<u8> } =
        @{ format!("The list of types ({letters:?}) has {expected} letters, but received {received} arguments.") },
    MissingTypeLetters { error: AmxError } = "missing the types-of-arguments string: {error}",
    MissingArrayLength = "an 'a' array argument must be followed by an 'i' or 'A' length",
    MissingArg { error: AmxError } = "missing or invalid argument: {error}",
    InvalidArrayLength { error: AmxError } = "invalid array length: {error}",
}
use ArgError::{
    InvalidArrayLength, MismatchedAmountOfArgs, MissingArg, MissingArrayLength, MissingTypeLetters,
};

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
    pub fn from_amx_args(mut args: VariadicArgs) -> Result<VariadicAmxArguments, ArgError> {
        let letters = args
            .next_arg::<AmxString>()
            .map_err(|error| MissingTypeLetters { error })?
            .to_bytes();
        let (expected, received) = (letters.len(), args.remaining());
        if expected != received {
            return Err(MismatchedAmountOfArgs { received, expected, letters });
        }

        let mut letters = letters.into_iter();
        let mut collected_arguments: Vec<PassedArgument> = Vec::with_capacity(expected);
        while let Some(type_letter) = letters.next() {
            collected_arguments.push(match type_letter {
                b's' => PassedArgument::Str(args.next_arg::<AmxString>().map_err(|error| MissingArg { error })?.to_bytes()),
                b'a' => PassedArgument::Array({
                    if !matches!(letters.next(), Some(b'i' | b'A')) {
                        return Err(MissingArrayLength);
                    }
                    let buffer: UnsizedBuffer = args.next_arg().map_err(|error| MissingArg { error })?;
                    let length: Ref<usize> = args.next_arg().map_err(|error| MissingArg { error })?;
                    let length = length.get().map_err(|error| InvalidArrayLength { error })?;
                    buffer
                        .into_sized_buffer(length).map_err(|error| InvalidArrayLength { error })?
                        .to_vec().map_err(|error| InvalidArrayLength { error })?
                }),
                _ => PassedArgument::PrimitiveCell(
                    args.next_arg::<Ref<RawCell>>().map_err(|error| MissingArg { error })?
                        .get().map_err(|error| MissingArg { error })?,
                ),
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
                PassedArgument::PrimitiveCell(cell) => {
                    amx.push(cell)?;
                }
                PassedArgument::Str(bytes) => {
                    amx.push(allocator.allot_bytes(bytes)?)?;
                }
                PassedArgument::Array(cells) => {
                    let length = i32::try_from(cells.len()).map_err(|_| AmxError::Domain)?;
                    let amx_buffer = allocator.allot_array(cells.as_slice())?;
                    amx.push(length)?;
                    amx.push(amx_buffer)?;
                }
            }
        }
        Ok(())
    }
}
