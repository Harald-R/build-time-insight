#pragma once

#include "vector3.h"

#include <vector>

// geometry.h depends on vector3.h only. main.cpp and shapes.h both include
// this header, so the analyzer sees it in two distinct inclusion chains.
namespace geom {

double distance(const Vector3<double> &a, const Vector3<double> &b);

Vector3<double> centroid(const std::vector<Vector3<double>> &points);

} // namespace geom