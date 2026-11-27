#include "shapes.h" // -> geometry.h -> vector3.h
#include "utils.h"

namespace geom {

double volume(const Sphere &s) {
  constexpr double pi = 3.14159265358979323846;
  return 4.0 / 3.0 * pi * s.radius * s.radius * s.radius;
}

double volume(const Box &b) {
  const auto d = b.max - b.min;
  return d.x() * d.y() * d.z();
}

double total_volume(const std::vector<Sphere> &spheres) {
  double total = 0.0;
  for (const auto &s : spheres)
    total += volume(s);
  return total;
}

} // namespace geom