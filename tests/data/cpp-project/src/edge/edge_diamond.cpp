#include "diamond_a.h" // diamond_a.h entered directly (first path)
#include "geometry.h"  // -> vector3.h (real work so the TU is non-trivial)

#include <vector>

// Includes diamond_a.h directly. Together with edge_diamond2.cpp (which
// reaches diamond_a.h only through diamond_b.h) this exercises different
// inclusion chains for the same header across units.
namespace geom {

double edge_diamond_work(const std::vector<Vector3<double>> &pts) {
  const DiamondA a;
  return centroid(pts).x() + static_cast<double>(a.v);
}

} // namespace geom

double edge_diamond_entry() { return geom::edge_diamond_work({{1, 0, 0}}); }