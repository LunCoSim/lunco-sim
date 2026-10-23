// Read saved CPU samples with Tracy's own file reader and symbol tables.
#include "TracyFileRead.hpp"
#include "TracyWorker.hpp"

#include <charconv>
#include <chrono>
#include <iomanip>
#include <iostream>
#include <map>
#include <memory>
#include <stdexcept>
#include <string_view>
#include <thread>

template <typename T> T number(const char* text) {
    T value;
    const std::string_view input(text);
    const auto result = std::from_chars(input.data(), input.data() + input.size(), value);
    if (result.ec != std::errc{} || result.ptr != input.data() + input.size()) {
        throw std::runtime_error("invalid integer argument: " + std::string(input));
    }
    return value;
}

int run(int argc, char** argv) {
    if (argc != 6) {
        throw std::runtime_error("usage: tracy-samples TRACE EXPECTED_PID TID_OR_0 BEGIN_NS END_NS");
    }
    const auto expected_pid = number<uint64_t>(argv[2]);
    const auto selected_tid = number<uint64_t>(argv[3]);
    const auto begin = number<int64_t>(argv[4]);
    const auto end = number<int64_t>(argv[5]);
    if (!expected_pid || begin >= end) {
        throw std::runtime_error("expected PID must be positive and BEGIN_NS must precede END_NS");
    }
    const auto file = std::unique_ptr<tracy::FileRead>(tracy::FileRead::Open(argv[1]));
    if (!file) throw std::runtime_error("cannot open capture");
    tracy::Worker worker(*file);
    if (worker.GetPid() != expected_pid) throw std::runtime_error("capture PID does not match owner");
    const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(60);
    while (!worker.AreCallstackSamplesReady()) {
        if (std::chrono::steady_clock::now() >= deadline) {
            throw std::runtime_error("Tracy sample processing did not complete within 60 seconds");
        }
        std::this_thread::sleep_for(std::chrono::milliseconds(10));
    }
    if (!worker.GetCallstackSampleCount()) throw std::runtime_error("capture has no CPU callstack samples");
    std::map<std::pair<uint64_t, uint32_t>, uint64_t> counts;
    uint64_t selected = 0;
    uint64_t no_stack = 0;
    for (const auto* thread : worker.GetThreadData()) {
        if (selected_tid && thread->id != selected_tid) continue;
        for (const auto& sample : thread->samples) {
            if (sample.time.Val() < begin || sample.time.Val() >= end) continue;
            ++selected;
            const auto stack = uint32_t(sample.callstack.Val());
            if (!stack) { ++no_stack; continue; }
            if (stack > worker.GetCallstackPayloadCount()) throw std::runtime_error("invalid sample callstack ID");
            ++counts[{thread->id, stack}];
        }
    }
    std::cerr << "pid=" << worker.GetPid() << " period_ns=" << worker.GetSamplingPeriod()
              << " capture_samples=" << worker.GetCallstackSampleCount()
              << " selected_samples=" << selected << " samples_without_stack=" << no_stack << '\n';
    if (counts.empty()) throw std::runtime_error("no CPU callstacks in the selected thread/time interval");
    std::cout << "pid,tid,csv_thread,thread,callstack,frame,inline_frame,samples,address,frame_metadata_present,function,file,line\n";
    for (const auto& [key, count] : counts) {
        const auto [tid, stack_id] = key;
        const auto csv_thread = worker.CompressThread(tid);
        const auto& stack = worker.GetCallstack(stack_id);
        size_t index = 0;
        for (const auto& entry : stack) {
            const auto* frame = worker.GetCallstackFrame(entry);
            const auto address = worker.GetCanonicalPointer(entry);
            const auto prefix = [&] {
                std::cout << expected_pid << ',' << tid << ',' << csv_thread << ','
                          << std::quoted(worker.GetThreadName(tid), '"', '"') << ','
                          << stack_id << ',' << index << ',';
            };
            if (!frame || !frame->size) {
                prefix();
                std::cout << ',' << count << ',' << address << ",0,,,\n";
            } else {
                for (uint8_t inlined = 0; inlined < frame->size; ++inlined) {
                    const auto& symbol = frame->data[inlined];
                    prefix();
                    std::cout << unsigned(inlined) << ',' << count << ',' << address << ",1,"
                              << std::quoted(worker.GetString(symbol.name), '"', '"') << ','
                              << std::quoted(worker.GetString(symbol.file), '"', '"') << ','
                              << symbol.line << '\n';
                }
            }
            ++index;
        }
    }
    if (!std::cout) throw std::runtime_error("sample export write failed");
    return 0;
}

int main(int argc, char** argv) {
    try { return run(argc, argv); }
    catch (const tracy::UnsupportedVersion& error) {
        std::cerr << "unsupported Tracy version: " << error.version << '\n';
    }
    catch (const tracy::LegacyVersion& error) {
        std::cerr << "legacy Tracy version: " << error.version << '\n';
    }
    catch (const tracy::LoadFailure& error) { std::cerr << "Tracy load failed: " << error.msg << '\n'; }
    catch (const std::exception& error) { std::cerr << "sample export failed: " << error.what() << '\n'; }
    return 1;
}
