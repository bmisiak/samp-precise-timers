# samp-precise-timers ⌚
Developed for [net4game.com](https://net4game.com) (RolePlay), this SA-MP plugin provides precise timers for the server. It is written in [Rust](https://rust-lang.org), a memory-safe language.

### Why rewrite timers?
I had a lot of safety concerns with some of the existing solutions. They weren't written with data integrity, memory safety or preventing server crashes in mind and seemed to have quite a few bugs. As privacy and safety is our primary concern at net4game, I wrote this in Rust, which provides high-level ergonomics and low-level performance. ⚡

Take a look at the code to see the benefits.

### Notes
* Calling `DeletePreciseTimer` from a timer's callback works fine. ✔
* Creating new timers from callbacks works fine as well. ✔


## Compiling
### Compile for Linux servers
Install Rust from [rustup.rs](https://rustup.rs). Afterwards, you are three commands away from being able to compile for SA-MP, which is a 32-bit application:
```
rustup toolchain install stable-i686-unknown-linux-gnu
rustup target add i686-unknown-linux-gnu
```
Then, enter the project directory and execute:
```
cargo +stable-i686-unknown-linux-gnu build --release
```
### Compile for Windows servers
**Note:** you might need Visual Studio Build Tools.

Install Rust from [rustup.rs](https://rustup.rs). Afterwards, open PowerShell and you are three commands away from being able to compile for SA-MP, which is a 32-bit application:
```
rustup toolchain install stable-i686-pc-windows-msvc
rustup target add i686-pc-windows-msvc
```
Then, enter the project directory and execute:
```
cargo +nightly-i686-pc-windows-msvc build --release
```
## Contributing
If you like the code and would like to help out, feel free to submit a pull request. Let me know at bm+code@net4game.com if you would like to join our team. 👋