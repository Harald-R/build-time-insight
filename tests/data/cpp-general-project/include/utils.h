#pragma once

#include <type_traits>

// Plain inline helper used by main.cpp (also gives the TU a bit of real work).
inline double radians(double deg) { return deg * 0.017453292519943295769; }

// Free function templates. Instantiated from several translation units so the
// analyzer reports a stable set of function-template instantiations.
namespace geom {

template <typename T> constexpr T clamp(T v, T lo, T hi) {
  return v < lo ? lo : (v > hi ? hi : v);
}

template <typename T> constexpr T lerp(T a, T b, T t) {
  return a + (b - a) * t;
}

} // namespace geom