#include "geometry.h" // -> vector3.h

#include <cmath>

namespace geom {

double distance(const Vector3<double> &a, const Vector3<double> &b) {
  return (a - b).length();
}

Vector3<double> centroid(const std::vector<Vector3<double>> &points) {
  Vector3<double> sum;
  for (const auto &p : points)
    sum += p;
  return sum;
}

} // namespace geom