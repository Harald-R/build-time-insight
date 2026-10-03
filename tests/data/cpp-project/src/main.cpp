#include "geometry.h" // -> vector3.h
#include "shapes.h"   // -> geometry.h -> vector3.h
#include "utils.h"

#include <iostream>
#include <vector>

// Exercises the project templates in this translation unit: Vector3<double>
// and Vector3<float> class instantiations plus the clamp/lerp/dot/operator
// function templates.
int main() {
  using namespace geom;

  std::vector<Vector3<double>> points = {
      {0, 0, 0},
      {1, 0, 0},
      {0, 1, 0},
      {0, 0, 1},
  };
  const auto c = centroid(points);

  const Sphere a{c, 2.0};
  const Box box{{-1, -1, -1}, {1, 1, 1}};

  const double t = clamp(0.5, 0.0, 1.0);
  const double blend = lerp(1.0, 2.0, t);
  const double half_turn = radians(180.0);

  Vector3<float> f{1.0f, 2.0f, 3.0f};
  const double d = dot(c, Vector3<double>{1, 1, 1});

  const std::vector<Sphere> spheres{a, {{0, 0, 0}, 1.0}, {{10, 10, 10}, 0.5}};

  std::cout << "centroid: " << c.x() << "," << c.y() << "," << c.z() << "\n"
            << "volumes: " << volume(a) << " " << volume(box) << " "
            << total_volume(spheres) << "\n"
            << "blend: " << blend << " length: " << f.length() << " dot: " << d
            << "\n";
  return 0;
}