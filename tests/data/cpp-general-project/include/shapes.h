#pragma once

#include "geometry.h" // -> vector3.h

#include <vector>

// shapes.h includes geometry.h, so its inclusion chain is
// shapes.h -> geometry.h -> vector3.h. Both shapes.cpp and main.cpp include
// this header.
namespace geom {

struct Sphere {
  Vector3<double> center;
  double radius = 1.0;
};

struct Box {
  Vector3<double> min;
  Vector3<double> max;
};

// An inline function definition: this gives the analyzer's header-parse
// accounting a ParseDeclarationOrFunctionDefinition event for shapes.h in
// every translation unit that includes it (struct definitions alone do not).
inline constexpr double radius_squared(const Sphere &s) {
  return s.radius * s.radius;
}

double volume(const Sphere &s);
double volume(const Box &b);
double total_volume(const std::vector<Sphere> &spheres);

} // namespace geom