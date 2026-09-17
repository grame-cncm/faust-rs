// A C++ host that fails to compile a program, through either wrapper.
//
// The C API's `error_msg` is 4096 bytes and carries the one-line summary of a
// compiler error. The wrappers' `std::string& error_msg` has no such limit and
// receives the complete text, read from getCComplete*DSPFactoryError: summary,
// then location, source snippet, notes and fix.
//
// Built and run by `cargo run -p xtask -- libfaust-export-check` when the Faust
// architecture headers are found (FAUST_ARCH_DIR, ../faust/architecture,
// /usr/local/include); by hand:
//
//   c++ -std=c++17 -I crates/cranelift-ffi/include -I crates/interp-ffi/include \
//       -I /path/to/faust/architecture -DWRAPPER_CRANELIFT \
//       crates/cranelift-ffi/tests/header-smoke/complete_error_cpp_client.cpp \
//       -L target/debug -lfaust-rs -Wl,-rpath,target/debug
//
// One wrapper per translation unit (-DWRAPPER_CRANELIFT or
// -DWRAPPER_INTERPRETER): each header is self-contained and declares the
// shared C symbols itself.

#include <iostream>
#include <string>

#if defined(WRAPPER_CRANELIFT)
#include "cranelift-dsp.h"
#define WRAPPER "cranelift"
static dsp_factory* from_string(const std::string& source, std::string& error_msg)
{
    return createCraneliftDSPFactoryFromString("client", source, 0, nullptr, error_msg, 0);
}
static std::string expand(const std::string& source, std::string& error_msg)
{
    std::string sha_key;
    return expandCraneliftDSPFromString("client", source, 0, nullptr, sha_key, error_msg);
}
#elif defined(WRAPPER_INTERPRETER)
#include "interpreter-dsp.h"
#define WRAPPER "interpreter"
static dsp_factory* from_string(const std::string& source, std::string& error_msg)
{
    return createInterpreterDSPFactoryFromString("client", source, 0, nullptr, error_msg);
}
static std::string expand(const std::string& source, std::string& error_msg)
{
    std::string sha_key;
    return expandInterpreterDSPFromString("client", source, 0, nullptr, sha_key, error_msg);
}
#else
#error "define WRAPPER_CRANELIFT or WRAPPER_INTERPRETER"
#endif

static int failures = 0;

static void expect(bool condition, const std::string& what, const std::string& text)
{
    if (!condition) {
        ++failures;
        std::cerr << WRAPPER << ": " << what << "\n--- error_msg (" << text.size()
                  << " bytes):\n" << text.substr(0, 600) << "\n---\n";
    }
}

static bool has(const std::string& text, const std::string& part)
{
    return text.find(part) != std::string::npos;
}

int main()
{
    // An unclosed parenthesis on line 2, column 21.
    const std::string unclosed = "// a comment line\nprocess = _ : *(0.5 ;\n";
    std::string error_msg;

    expect(from_string(unclosed, error_msg) == nullptr, "the program compiled", error_msg);
    expect(error_msg.rfind("parse failed for", 0) == 0, "the summary comes first", error_msg);
    expect(has(error_msg, ":2:21: error [FRS-PARSE-0001]"), "the location", error_msg);
    expect(has(error_msg, "  2 | process = _ : *(0.5 ;"), "the source line", error_msg);
    expect(has(error_msg, "insert `)`"), "the fix", error_msg);
    expect(error_msg.back() != '\n', "no trailing newline", error_msg);

    // More than the C buffer holds: an undefined symbol lists the visible scope.
    std::string many;
    for (int i = 0; i < 200; ++i) {
        many += "a_rather_long_definition_name_" + std::to_string(i) + " = " + std::to_string(i) + ";\n";
    }
    many += "process = _ : missing_symbol;\n";
    expect(from_string(many, error_msg) == nullptr, "the program compiled", error_msg);
    expect(error_msg.size() > 4096, "a text over 4096 bytes is whole", std::to_string(error_msg.size()));
    expect(has(error_msg, "undefined symbol `missing_symbol`"), "the undefined symbol", error_msg);
    expect(has(error_msg, "a_rather_long_definition_name_199"), "the end of the scope", error_msg);

    // A success clears the string; the failure after it reports itself.
    dsp_factory* factory = from_string("process = _;", error_msg);
    expect(factory != nullptr && error_msg.empty(), "a success leaves no message", error_msg);
    expect(expand(unclosed, error_msg).empty(), "the expansion succeeded", error_msg);
    expect(has(error_msg, "[FRS-PARSE-0001]") && !has(error_msg, "missing_symbol"),
           "an expansion failure carries its own diagnostic", error_msg);

    if (failures == 0) std::cout << WRAPPER << ": ok" << std::endl;
    return failures == 0 ? 0 : 1;
}
