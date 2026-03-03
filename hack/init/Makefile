BUTANE ?= butane

%.ign: %.bu
	$(BUTANE) --strict --files-dir ../.. < $< > $@

config.ign: base.ign users.ign
	$(BUTANE) --strict --files-dir . < config.bu > $@

.PHONY: clean
clean:
	rm -f *.ign
