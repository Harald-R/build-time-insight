module;
#include <string>
#include <vector>
export module core;

// Re-export a partition interface. The internal partition is used by the
// implementation unit instead.
export import :shapes;

namespace core {

export template <typename T>
struct Vec {
    T x{};
    T y{};

    T sum() const { return x + y; }
};

export using Ints = std::vector<int>;

export int add(int a, int b);
export std::string label(int value);
export int use_detail(int value);
export Ints build(int count);

// Force a Vec<double> instantiation while compiling the interface unit.
export inline double instantiated_sum() { return Vec<double>{1.0, 2.0}.sum(); }

} // namespace core
