#pragma once

#include <string>

namespace util {

inline std::string tag(int value) { return "v" + std::to_string(value); }

template <typename T>
T clamp(T value, T low, T high) {
    return value < low ? low : (value > high ? high : value);
}

} // namespace util
