#include "diamond_b.h" // -> diamond_a.h (diamond_a.h reached indirectly)
#include "geometry.h"  // -> vector3.h (real work so the TU is non-trivial)

#include <vector>

// Reaches diamond_a.h only via diamond_b.h, so the analyzer records a
// diamond_b.h -> diamond_a.h inclusion chain for this unit, distinct from the
// direct path taken in edge_diamond.cpp.
namespace geom {

double edge_diamond2_work(const std::vector<Vector3<double>> &pts) {
  const DiamondB b;
  return centroid(pts).x() + static_cast<double>(b.v);
}

} // namespace geom

double edge_diamond2_entry() { return geom::edge_diamond2_work({{1, 0, 0}}); }