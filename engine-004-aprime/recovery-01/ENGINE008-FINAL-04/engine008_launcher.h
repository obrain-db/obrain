#pragma once
#include <string>
#include <vector>
struct LaunchConfig;
struct LaunchReport;
LaunchReport run_sequence(const LaunchConfig & c);
const char * verdict_name(RunVerdict v);
