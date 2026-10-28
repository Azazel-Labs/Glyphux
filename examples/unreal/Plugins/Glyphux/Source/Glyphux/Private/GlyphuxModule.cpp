#include "Modules/ModuleManager.h"
#include "Ferreal/FerrealGlyphux.h"

class FGlyphuxModule : public IModuleInterface
{
public:
	virtual void StartupModule() override
	{
		// Hands Rust its tables of C++ functions (the classes would on first use anyway).
		Ferreal::Glyphux::Register();
	}
};

IMPLEMENT_MODULE(FGlyphuxModule, Glyphux)
