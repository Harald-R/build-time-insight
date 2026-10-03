#pragma once

#include <array>
#include <cmath>
#include <cstddef>

// A small vector template. Instantiated with several element types across the
// translation units so the analyzer sees a repeatable set of class-template
// instantiations with a known unit distribution.
namespace geom {

template <typename T> class Vector3 {
public:
  constexpr Vector3(T x = T{}, T y = T{}, T z = T{}) : data_{x, y, z} {}

  T x() const { return data_[0]; }
  T y() const { return data_[1]; }
  T z() const { return data_[2]; }

  Vector3 &operator+=(const Vector3 &o) {
    for (std::size_t i = 0; i < 3; ++i)
      data_[i] += o.data_[i];
    return *this;
  }
  Vector3 &operator-=(const Vector3 &o) {
    for (std::size_t i = 0; i < 3; ++i)
      data_[i] -= o.data_[i];
    return *this;
  }

  T length() const {
    T s = T{};
    for (T v : data_)
      s += v * v;
    return std::sqrt(s);
  }

private:
  std::array<T, 3> data_;
};

template <typename T> Vector3<T> operator+(Vector3<T> a, const Vector3<T> &b) {
  a += b;
  return a;
}

template <typename T> Vector3<T> operator-(Vector3<T> a, const Vector3<T> &b) {
  a -= b;
  return a;
}

template <typename T> T dot(const Vector3<T> &a, const Vector3<T> &b) {
  return a.x() * b.x() + a.y() * b.y() + a.z() * b.z();
}

} // namespace geom