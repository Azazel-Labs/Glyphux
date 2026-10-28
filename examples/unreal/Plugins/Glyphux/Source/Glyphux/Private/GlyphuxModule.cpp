#include "Modules/ModuleManager.h"
#include "Rivet/RivetGlyphux.h"

class FGlyphuxModule : public IModuleInterface
{
public:
	virtual void StartupModule() override
	{
		// Hands Rust its tables of C++ functions (the classes would on first use anyway).
		Rivet::Glyphux::Register();
	}
};

IMPLEMENT_MODULE(FGlyphuxModule, Glyphux)
