# samp-precise-timers ⌚
Developed for [net4game.com](https://net4game.com) (RolePlay), this SA-MP plugin provides precise timers for the server. It is written in [Rust](https://rust-lang.org), a memory-safe language.


## Installation
Simply install to your project:
```bash
sampctl package install bmisiak/samp-precise-timers
```
Include in your code and begin using the library:

```pawn
#include <samp-precise-timers>
```

### Why rewrite timers?
I had a lot of safety concerns with some of the existing solutions. They weren't written with data integrity, memory safety or preventing server crashes in mind and seemed to have quite a few bugs. This is written in Rust, which combines high-level ergonomics with the performance of a low-level language. ⚡

Take a look at the code to see the benefits.

### Notes
* Calling `DeletePreciseTimer` from a timer's callback works fine. ✔
* Creating new timers from callbacks works fine as well. ✔
* Supports strings and arrays properly without memory leaks. ✔

## Compiling
Install Rust from [rustup.rs](https://rustup.rs). Afterwards, you are two commands away from being able to compile for SA-MP, which is a 32-bit application.
### Compile for Linux servers

```
rustup target add i686-unknown-linux-gnu
```
Then, enter the project directory and execute:
```
cargo build --target=i686-unknown-linux-gnu --release
```
### Compile for Windows servers
**Note:** You might need to install **Visual Studio Build Tools**.
```
rustup target add i686-pc-windows-msvc
```
Then, enter the project directory and execute:
```
cargo build --target=i686-pc-windows-msvc --release
```
## Contributing
If you like the code and would like to help out, feel free to submit a pull request. Let me know at bm+code@net4game.com if you would like to join our team. 👋
